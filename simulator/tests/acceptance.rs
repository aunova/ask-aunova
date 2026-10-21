//! What the simulator must actually do, proved against the real server.
//!
//! Nothing here is mocked. Each test builds no MCP traffic of its own: it
//! starts the Ask Aunova server from its built binary on a free loopback port,
//! points a real [`Simulator`] at it, and asks a question through the same
//! code path the page uses. The tool calls are real `tools/call` requests over
//! Streamable HTTP and the results are the server's own.
//!
//! The server binary is not built from here — this crate is its own workspace
//! and building the other one from a test would be a surprise. Build it first
//! (`cargo build -p ask-aunova-server` from the repository root) or point
//! [`SERVER_BIN_ENV`] at it; a missing binary fails with that instruction
//! rather than a silent skip.
//!
//! Every server is stopped by the exact process id this test started, never by
//! name or pattern.

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::Duration,
};

use ask_aunova_simulator::{
    endpoint::Endpoint,
    identity::Identity,
    model::Standing,
    stub::{SCRIPTED_QUESTIONS, StubModel},
    transcribe::{FIXTURES, StubTranscriber, fixture_pcm},
    turn::Turn,
    unit_id::{PROJECT_NAME, UNITS},
    voice::SilentVoice,
    web::Simulator,
};

/// Environment variable naming the server binary to start.
const SERVER_BIN_ENV: &str = "ASK_AUNOVA_SERVER_BIN";

/// Where the repository's own build puts it, relative to this crate.
const DEFAULT_SERVER_BIN: &str = "../target/debug/ask-aunova-server";

/// How long to wait for the server to say which port it bound.
const STARTUP_DEADLINE: Duration = Duration::from_secs(30);

/// Largest answer this test will read back over a socket.
const MAX_RESPONSE_BYTES: u64 = 1024 * 1024;

/// A child process started for one test, and stopped with it.
///
/// By the exact process id this test started, and never by name or pattern.
struct Stopped(Child);

impl Drop for Stopped {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A server process started for one test, and stopped with it.
struct ServerUnderTest {
    _child: Stopped,
    endpoint: Endpoint,
}

impl ServerUnderTest {
    /// Start the server on a free loopback port and wait until it says so.
    ///
    /// The port is read back out of the server's own startup line rather than
    /// guessed: asking for port 0 and being told what was bound cannot race
    /// with another test picking the same number.
    fn start() -> Self {
        let named = PathBuf::from(
            std::env::var(SERVER_BIN_ENV).unwrap_or_else(|_| DEFAULT_SERVER_BIN.to_owned()),
        );
        assert!(
            named.is_file(),
            "no server binary at {}. Build it (`cargo build -p ask-aunova-server` from the \
             repository root) or set {SERVER_BIN_ENV}.",
            named.display(),
        );
        // Resolved before the child's working directory is changed: a
        // relative program path is looked up *after* the chdir, so
        // `../target/debug/...` would be read against the repository root and
        // point outside it.
        let binary = named
            .canonicalize()
            .unwrap_or_else(|error| panic!("{} cannot be resolved: {error}", named.display()));

        let mut child = Command::new(&binary)
            // The server resolves its dataset and key directories relative to
            // where it was started, and the repository root is where its
            // defaults point.
            .current_dir("..")
            .env("ASK_AUNOVA_PORT", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|error| panic!("could not start {}: {error}", binary.display()));

        let url = listening_on(&mut child, "server");
        let endpoint = Endpoint::parse(&url).expect("the server binds loopback");
        Self {
            _child: Stopped(child),
            endpoint,
        }
    }

    /// A simulator pointed at this server, on the stub model and silent voice.
    fn simulator(&self) -> Simulator {
        Simulator::new(
            self.endpoint.clone(),
            Box::new(StubModel),
            Box::new(SilentVoice),
            Box::new(StubTranscriber),
        )
    }
}

/// Drain a child's output, and return the loopback URL it says it bound.
///
/// Both streams are drained for the life of the process: a full pipe would
/// block it mid-answer, which would look like a timeout.
fn listening_on(child: &mut Child, what: &str) -> String {
    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");
    let (bound, found) = mpsc::channel();

    std::thread::spawn(move || {
        let mut announced = false;
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if !announced && let Some(url) = listening_url(&line) {
                announced = bound.send(url).is_ok();
            }
        }
    });
    std::thread::spawn(
        move || {
            for _line in BufReader::new(stderr).lines().map_while(Result::ok) {}
        },
    );

    match found.recv_timeout(STARTUP_DEADLINE) {
        Ok(url) => url,
        Err(reason) => {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the {what} never said which port it bound ({reason})");
        }
    }
}

/// The MCP URL in the server's startup line, if that is what this line is.
fn listening_url(line: &str) -> Option<String> {
    let rest = line.split_once("listening on ")?.1;
    let url = rest
        .split_whitespace()
        .next()?
        .trim_end_matches(|character: char| !character.is_ascii_alphanumeric() && character != '/');
    url.starts_with("http://").then(|| url.to_owned())
}

/// Every document id the answer cited.
fn citations(turn: &Turn) -> Vec<String> {
    turn.answer.citations.clone()
}

/// The tools that were actually called, in order.
fn tools_called(turn: &Turn) -> Vec<String> {
    turn.calls.iter().map(|call| call.tool.clone()).collect()
}

#[tokio::test]
async fn a_compound_question_makes_two_tool_calls_in_one_turn_and_cites_documents() {
    let server = ServerUnderTest::start();
    let simulator = server.simulator();

    let turn = simulator
        .ask(Identity::Buyer, SCRIPTED_QUESTIONS[0].question)
        .await
        .expect("the compound question is answered");

    assert!(
        turn.calls.len() >= 2,
        "a compound question must make two or more tools/call in one turn, not {}: {:?}",
        turn.calls.len(),
        tools_called(&turn),
    );
    assert_eq!(
        tools_called(&turn),
        vec!["get_milestones", "get_certificates"],
        "both halves of the question must be asked",
    );
    assert!(
        turn.calls.iter().all(|call| !call.refused),
        "neither half of this question is outside a buyer's scope",
    );

    // The hackathon claim is that this page is a real MCP client, so the
    // handshake and the advertised tool list are part of the acceptance.
    assert_eq!(turn.server.protocol_version, "2025-11-25");
    assert_eq!(
        turn.tools_advertised.len(),
        6,
        "tools/list advertised {:?}",
        turn.tools_advertised,
    );

    assert!(
        !turn.answer.statements.is_empty(),
        "an answer with no statements has nothing in it",
    );
    let cited = citations(&turn);
    assert!(
        !cited.is_empty(),
        "the composed answer must cite document ids: {}",
        turn.answer.spoken,
    );
    for doc_id in &cited {
        assert!(
            turn.answer.spoken.contains(doc_id.as_str()),
            "{doc_id} is cited but never said: {}",
            turn.answer.spoken,
        );
    }

    // Every sentence came from a call this turn made, which is the honesty
    // rule the loop exists to enforce.
    for statement in &turn.answer.statements {
        let call = turn
            .calls
            .get(statement.from_call)
            .unwrap_or_else(|| panic!("statement names call {}", statement.from_call));
        // The escaped, quoted form of the sentence has to appear in the
        // serialised result: comparing the two in JSON's own encoding means
        // a quote or a newline inside a sentence cannot make the check pass
        // or fail by accident.
        let quoted = serde_json::to_string(&statement.text).expect("a sentence serialises");
        let recorded = serde_json::to_string(&call.result).expect("a result serialises");
        assert!(
            recorded.contains(&quoted),
            "`{}` was spoken but is not in the result of `{}`",
            statement.text,
            call.tool,
        );
    }
}

#[tokio::test]
async fn the_buyer_token_is_refused_unit_0802_and_the_refusal_is_the_answer() {
    let server = ServerUnderTest::start();
    let simulator = server.simulator();
    let question = SCRIPTED_QUESTIONS[2].question;
    assert!(
        question.contains("0802"),
        "this test is about the unit the buyer token is not bound to: {question}",
    );

    let refused = simulator
        .ask(Identity::Buyer, question)
        .await
        .expect("a refusal is an answer, not a failure");

    assert!(
        refused.calls.iter().any(|call| call.refused),
        "the server must refuse this unit to a buyer: {:?}",
        tools_called(&refused),
    );
    assert!(
        refused
            .answer
            .statements
            .iter()
            .any(|statement| statement.standing == Standing::Refused),
        "the refusal must reach the person as a refusal: {}",
        refused.answer.spoken,
    );
    assert!(
        refused
            .answer
            .spoken
            .starts_with("The server refused that."),
        "a refusal is never spoken as a finding: {}",
        refused.answer.spoken,
    );
    assert!(
        refused
            .answer
            .statements
            .iter()
            .all(|statement| statement.standing == Standing::Refused),
        "nothing else may be stated once the call was refused: {}",
        refused.answer.spoken,
    );
    // 0802's own figures, from the synthetic dataset. None of them may appear.
    for withheld in ["89100000", "198000000", "1,980,000", "891,000"] {
        assert!(
            !refused.answer.spoken.contains(withheld),
            "a refusal leaked {withheld}: {}",
            refused.answer.spoken,
        );
    }

    // The same question, the same client code, the same request body: only
    // the bearer token differs, and the server answers it.
    let allowed = simulator
        .ask(Identity::Developer, question)
        .await
        .expect("the developer may read any unit");
    assert!(
        allowed.calls.iter().all(|call| !call.refused),
        "the developer must not be refused: {}",
        allowed.answer.spoken,
    );
    assert!(
        !allowed.answer.citations.is_empty(),
        "the answered form must cite documents: {}",
        allowed.answer.spoken,
    );

    // The client never sends a role. If it did, this is where it would show.
    for turn in [&refused, &allowed] {
        for call in &turn.calls {
            let arguments = call.arguments.as_object().expect("arguments are an object");
            for name in arguments.keys() {
                assert!(
                    !name.contains("role") && !name.contains("token"),
                    "`{name}` was sent to `{}`; the role is the server's to decide",
                    call.tool,
                );
            }
        }
    }
}

#[tokio::test]
async fn a_repeated_amount_reaches_verify_claim_as_an_exact_count_of_minor_units() {
    let server = ServerUnderTest::start();
    let simulator = server.simulator();

    let turn = simulator
        .ask(Identity::Buyer, SCRIPTED_QUESTIONS[3].question)
        .await
        .expect("a claim about one's own unit is checkable");

    assert_eq!(tools_called(&turn), vec!["verify_claim"]);
    let sent = turn.calls[0]
        .arguments
        .get("asserted_value")
        .and_then(serde_json::Value::as_str);
    assert_eq!(
        sent,
        Some("147000000"),
        "AED 1,470,000.00 is 147000000 fils; any other form is UNVERIFIED, not a verdict",
    );
    assert_eq!(
        turn.calls[0]
            .result
            .get("verdict")
            .and_then(serde_json::Value::as_str),
        Some("SUPPORTED"),
        "a canonical value must reach a verdict: {:?}",
        turn.calls[0].result.get("reason"),
    );
    assert!(
        !turn.answer.citations.is_empty(),
        "a supported claim is stated with the documents it was compared against",
    );
}

#[tokio::test]
async fn an_approximate_amount_asks_which_figure_and_calls_nothing() {
    let server = ServerUnderTest::start();
    let simulator = server.simulator();

    let turn = simulator
        .ask(Identity::Buyer, SCRIPTED_QUESTIONS[4].question)
        .await
        .expect("a question back is an answer");

    assert!(
        turn.calls.is_empty(),
        "an approximate figure must not reach the server: {:?}",
        tools_called(&turn),
    );
    assert!(turn.answer.is_clarification);
    assert!(
        turn.answer.spoken.contains("exact figure"),
        "{}",
        turn.answer.spoken,
    );
    assert!(turn.answer.citations.is_empty());
}

#[tokio::test]
async fn an_evidence_card_is_read_over_mcp_as_a_resource() {
    let server = ServerUnderTest::start();
    let simulator = server.simulator();

    let turn = simulator
        .ask(Identity::Buyer, SCRIPTED_QUESTIONS[0].question)
        .await
        .expect("the compound question is answered");
    let doc_id = citations(&turn)
        .into_iter()
        .next()
        .expect("the answer cited at least one document");

    let card = simulator
        .evidence(Identity::Buyer, &doc_id)
        .await
        .unwrap_or_else(|error| panic!("the card for {doc_id} is served: {error}"));

    assert!(
        card.contains(&doc_id),
        "the card names the document it is for"
    );
    assert!(
        card.contains("SYNTHETIC DEMO DATA"),
        "every rendered card carries the watermark",
    );
    assert!(card.starts_with("<!DOCTYPE html>"));
}

#[tokio::test]
async fn a_question_the_stub_has_no_rule_for_is_said_to_be_one() {
    let server = ServerUnderTest::start();
    let simulator = server.simulator();

    let error = simulator
        .ask(Identity::Buyer, "what is the weather in the lobby")
        .await
        .expect_err("the stub does not improvise");
    let message = error.to_string();
    assert!(
        message.contains("no rule"),
        "the stub must say it has no rule rather than inventing one: {message}",
    );
}

#[tokio::test]
async fn the_page_is_served_over_loopback_and_references_nothing_remote() {
    // No MCP server is needed: the page is compiled into this binary and the
    // browser fetches it before it asks anything.
    let simulator = Simulator::new(
        Endpoint::parse("http://127.0.0.1:8731/mcp").expect("loopback"),
        Box::new(StubModel),
        Box::new(SilentVoice),
        Box::new(StubTranscriber),
    );
    let (addr, serving) = serve(simulator).await;

    let page = tokio::task::spawn_blocking(move || get(addr, "/"))
        .await
        .expect("the fetch task runs");
    serving.abort();

    assert_eq!(page.status, 200);
    assert!(page.body.contains("<!DOCTYPE html>"), "{}", page.body);
    assert!(
        !page.body.contains("://"),
        "the served page must carry no absolute URL at all",
    );
    assert!(page.body.contains("<b>Simulator</b>"));
}

#[tokio::test]
async fn a_recording_drives_the_whole_chain_to_an_answer_that_cites_documents() {
    let server = ServerUnderTest::start();
    let (addr, serving) = serve(server.simulator()).await;

    // 1. The recording, posted exactly as the page posts it: raw 16 kHz mono
    //    signed 16-bit little-endian PCM, with no container around it.
    let recording = fixture_bytes("payment-unit-1204.pcm");
    let heard = tokio::task::spawn_blocking(move || {
        post(
            addr,
            "/api/transcribe",
            "application/octet-stream",
            &recording,
        )
    })
    .await
    .expect("the post runs");
    assert_eq!(heard.status, 200, "{}", heard.body);
    let heard = heard.json();

    assert_eq!(
        heard["interpretation"]["heard"][0],
        "what is the payment status of unit twelve oh four",
    );
    assert_eq!(heard["interpretation"]["outcome"], "resolved");
    assert_eq!(
        heard["interpretation"]["choices"][0]["reference"]["id"],
        "1204"
    );
    let question = heard["interpretation"]["question"]
        .as_str()
        .unwrap_or_else(|| panic!("a resolved reading offers a question: {heard}"))
        .to_owned();
    assert!(
        question.contains("unit 1204"),
        "the spoken number must be written out before the loop reads it: {question}",
    );

    // 2. The question the person saw and confirmed, asked over the same HTTP
    //    face the page uses.
    let asked = serde_json::json!({ "identity": "buyer", "question": question }).to_string();
    let answered = tokio::task::spawn_blocking(move || {
        post(addr, "/api/ask", "application/json", asked.as_bytes())
    })
    .await
    .expect("the post runs");
    serving.abort();
    assert_eq!(answered.status, 200, "{}", answered.body);
    let turn = answered.json();

    let tools: Vec<&str> = turn["calls"]
        .as_array()
        .expect("a turn records its calls")
        .iter()
        .filter_map(|call| call["tool"].as_str())
        .collect();
    assert_eq!(tools, vec!["get_payment_status", "ask_project"], "{turn}");
    assert!(
        turn["calls"]
            .as_array()
            .is_some_and(|calls| calls.iter().all(|call| call["refused"] == false)),
        "the buyer's own unit is not refused: {turn}",
    );

    let citations: Vec<&str> = turn["answer"]["citations"]
        .as_array()
        .expect("an answer carries its citations")
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    assert!(
        citations.contains(&"PS-1204"),
        "the answer must cite the unit's own payment schedule: {citations:?}",
    );
    let spoken = turn["answer"]["spoken"].as_str().unwrap_or_default();
    for doc_id in &citations {
        assert!(
            spoken.contains(doc_id),
            "{doc_id} is cited but never said: {spoken}",
        );
    }
}

#[tokio::test]
async fn the_buyer_is_refused_unit_0802_through_the_voice_path_too() {
    let server = ServerUnderTest::start();
    let (addr, serving) = serve(server.simulator()).await;

    let recording = fixture_bytes("payment-unit-0802.pcm");
    let heard = tokio::task::spawn_blocking(move || {
        post(
            addr,
            "/api/transcribe",
            "application/octet-stream",
            &recording,
        )
    })
    .await
    .expect("the post runs");
    assert_eq!(heard.status, 200, "{}", heard.body);
    let heard = heard.json();

    // Two alternatives, one of which resolves to nothing: the units they
    // agree on are still one unit, and it is still confirmed before it is
    // asked.
    assert_eq!(
        heard["interpretation"]["heard"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        2,
        "{heard}",
    );
    assert_eq!(
        heard["interpretation"]["choices"][0]["reference"]["id"],
        "0802"
    );
    let question = heard["interpretation"]["question"]
        .as_str()
        .unwrap_or_else(|| panic!("a resolved reading offers a question: {heard}"))
        .to_owned();

    let asked = serde_json::json!({ "identity": "buyer", "question": question }).to_string();
    let answered = tokio::task::spawn_blocking(move || {
        post(addr, "/api/ask", "application/json", asked.as_bytes())
    })
    .await
    .expect("the post runs");
    serving.abort();
    // A refusal is an answer, so it comes back 200 with the refusal in it.
    assert_eq!(answered.status, 200, "{}", answered.body);
    let turn = answered.json();

    assert!(
        turn["calls"]
            .as_array()
            .is_some_and(|calls| calls.iter().any(|call| call["refused"] == true)),
        "the server must refuse a unit this token is not bound to: {turn}",
    );
    let statements = turn["answer"]["statements"]
        .as_array()
        .expect("an answer carries its statements");
    assert!(
        !statements.is_empty(),
        "a refusal still reaches the person: {turn}"
    );
    assert!(
        statements
            .iter()
            .all(|statement| statement["standing"] == "refused"),
        "nothing may be stated as a finding once the call was refused: {turn}",
    );
    let spoken = turn["answer"]["spoken"].as_str().unwrap_or_default();
    assert!(
        spoken.starts_with("The server refused that."),
        "the refusal must reach the person as a refusal: {spoken}",
    );
    // 0802's own figures, from the synthetic dataset. None of them may
    // appear because the question arrived by voice rather than by typing.
    for withheld in ["89100000", "198000000", "1,980,000", "891,000"] {
        assert!(
            !spoken.contains(withheld),
            "a refusal leaked {withheld}: {spoken}"
        );
    }
}

#[tokio::test]
async fn a_recording_that_names_the_project_resolves_to_the_project_and_not_to_a_unit() {
    // No MCP server: this is the reading, and the reading happens before
    // anything is asked of one.
    let simulator = Simulator::new(
        Endpoint::parse("http://127.0.0.1:8731/mcp").expect("loopback"),
        Box::new(StubModel),
        Box::new(SilentVoice),
        Box::new(StubTranscriber),
    );
    let (addr, serving) = serve(simulator).await;

    let recording = fixture_bytes("project-on-track.pcm");
    let heard = tokio::task::spawn_blocking(move || {
        post(
            addr,
            "/api/transcribe",
            "application/octet-stream",
            &recording,
        )
    })
    .await
    .expect("the post runs");
    serving.abort();
    assert_eq!(heard.status, 200, "{}", heard.body);
    let heard = heard.json();

    assert_eq!(heard["interpretation"]["outcome"], "resolved");
    assert_eq!(
        heard["interpretation"]["choices"][0]["reference"]["kind"],
        "project",
    );
    let question = heard["interpretation"]["question"]
        .as_str()
        .unwrap_or_else(|| panic!("a project question is still a question: {heard}"));
    for unit in UNITS {
        assert!(
            !question.contains(unit),
            "a question that named no unit must not acquire one: {question}",
        );
    }
}

#[tokio::test]
async fn a_recording_this_build_cannot_transcribe_is_refused_rather_than_invented() {
    let simulator = Simulator::new(
        Endpoint::parse("http://127.0.0.1:8731/mcp").expect("loopback"),
        Box::new(StubModel),
        Box::new(SilentVoice),
        Box::new(StubTranscriber),
    );
    let (addr, serving) = serve(simulator).await;

    // Silence of exactly a fixture's length: the one recording a stub that
    // matched on size alone would answer with somebody else's sentence.
    let silence = vec![0u8; fixture_bytes("payment-unit-1204.pcm").len()];
    let refused = tokio::task::spawn_blocking(move || {
        post(
            addr,
            "/api/transcribe",
            "application/octet-stream",
            &silence,
        )
    })
    .await
    .expect("the post runs");
    serving.abort();

    assert_eq!(refused.status, 422, "{}", refused.body);
    assert!(
        refused.body.contains("nothing was guessed"),
        "the stub must say it has no transcript rather than invent one: {}",
        refused.body,
    );
}

#[tokio::test]
async fn a_claim_the_record_contradicts_is_said_to_be_wrong_and_the_record_is_stated() {
    let server = ServerUnderTest::start();
    let simulator = server.simulator();
    let question = SCRIPTED_QUESTIONS[5].question;
    assert!(
        question.contains("1,500,000.00"),
        "this test is about a figure the record does not hold: {question}",
    );

    let turn = simulator
        .ask(Identity::Buyer, question)
        .await
        .expect("a claim about one's own unit is checkable");

    assert_eq!(tools_called(&turn), vec!["verify_claim"]);
    assert_eq!(
        turn.calls[0]
            .arguments
            .get("asserted_value")
            .and_then(serde_json::Value::as_str),
        Some("150000000"),
        "the figure the person stated goes to the server exactly, in minor units",
    );
    assert_eq!(
        turn.calls[0]
            .result
            .get("verdict")
            .and_then(serde_json::Value::as_str),
        Some("CONTRADICTED"),
        "{:?}",
        turn.calls[0].result,
    );

    // The server states both verdicts in the same sentence. This is the
    // whole of the difference between "yes" and "no" reaching the person.
    assert_eq!(turn.answer.statements[0].standing, Standing::Contradicted);
    assert!(
        turn.answer
            .spoken
            .starts_with("That is not what the record says."),
        "a contradiction must say so before anything else: {}",
        turn.answer.spoken,
    );
    // And it says what the record does hold, as money rather than as a count
    // of the minor units the record stores.
    assert!(
        turn.answer
            .spoken
            .contains("one million four hundred seventy thousand dirhams"),
        "the recorded figure has to be stated, in dirhams: {}",
        turn.answer.spoken,
    );
    assert!(
        !turn.answer.spoken.contains("147000000"),
        "nobody hears a count of fils as money: {}",
        turn.answer.spoken,
    );
    assert!(
        !turn.answer.citations.is_empty(),
        "a finding is stated with the documents it was compared against",
    );
    // The sentence that was checked is still the server's own, character for
    // character, under the one that is said.
    assert_eq!(
        turn.answer.statements[0].text,
        "The signed record states `paid_to_date_minor` for unit 1204 as `147000000`.",
    );
}

#[tokio::test]
async fn a_proxy_variable_in_the_environment_never_sees_the_bearer_token() {
    // `reqwest` reads `HTTP_PROXY` out of the environment unless it is told
    // not to, and every MCP request this client makes carries a bearer token
    // in an `Authorization` header. So: start a socket that pretends to be
    // that proxy, start the simulator with the environment pointing at it,
    // and ask a question. The answer must come back from the server, and the
    // proxy must never have been connected to at all.
    let sink = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port is free");
    let proxy = format!(
        "http://{}",
        sink.local_addr().expect("the sink has an address")
    );
    let (connected, reached) = mpsc::channel();
    std::thread::spawn(move || {
        for stream in sink.incoming() {
            if connected.send(stream.is_ok()).is_err() {
                return;
            }
        }
    });

    let server = ServerUnderTest::start();
    let mut child = Command::new(env!("CARGO_BIN_EXE_ask-aunova-simulator"))
        .env("ASK_AUNOVA_MCP_URL", server.endpoint.url())
        .env("ASK_AUNOVA_SIMULATOR_PORT", "0")
        .env("HTTP_PROXY", &proxy)
        .env("HTTPS_PROXY", &proxy)
        .env("ALL_PROXY", &proxy)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the simulator binary is built alongside this test");
    let url = listening_on(&mut child, "simulator");
    let simulator = Stopped(child);

    let addr: SocketAddr = url
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .parse()
        .unwrap_or_else(|error| panic!("`{url}` is the address it bound: {error}"));
    let asked = serde_json::json!({
        "identity": "buyer",
        "question": SCRIPTED_QUESTIONS[1].question,
    })
    .to_string();
    let answered = tokio::task::spawn_blocking(move || {
        post(addr, "/api/ask", "application/json", asked.as_bytes())
    })
    .await
    .expect("the post runs");
    drop(simulator);

    assert_eq!(answered.status, 200, "{}", answered.body);
    let turn = answered.json();
    assert!(
        !turn["answer"]["citations"]
            .as_array()
            .is_none_or(Vec::is_empty),
        "the question was answered by the server, not by a proxy: {turn}",
    );
    assert!(
        reached.try_recv().is_err(),
        "a proxy named only by an environment variable was connected to; a bearer token went \
         with it",
    );
}

#[test]
fn the_committed_fixtures_are_the_bytes_this_crate_generates() {
    // The recordings are generated, not recorded: a square wave in integer
    // arithmetic, so the committed file and the table the stub matches
    // against cannot drift apart. A fixture that is missing is written from
    // the generator and this test says so, rather than being quietly skipped.
    let mut written: Vec<String> = Vec::new();
    for fixture in &FIXTURES {
        let path = PathBuf::from(FIXTURE_DIR).join(fixture.name);
        let generated = fixture_pcm(fixture);
        match std::fs::read(&path) {
            Ok(committed) => assert_eq!(
                committed,
                generated,
                "{} is not what the generator produces",
                path.display(),
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir_all(FIXTURE_DIR).expect("the fixture directory can be made");
                std::fs::write(&path, &generated).expect("the fixture can be written");
                written.push(path.display().to_string());
            }
            Err(error) => panic!("{} cannot be read: {error}", path.display()),
        }
    }
    assert!(
        written.is_empty(),
        "these were missing and have been written from the generator; commit them and run this \
         again: {written:?}",
    );
}

#[test]
fn the_units_this_client_can_resolve_are_the_units_the_dataset_carries() {
    // The normaliser refuses a unit this project does not have, which is
    // only true while its list is the dataset's list.
    let record = std::fs::read_to_string("../data/palm-vista/project.json")
        .expect("the dataset is beside this crate");
    let record: serde_json::Value =
        serde_json::from_str(&record).expect("the project record is JSON");

    let mut units: Vec<&str> = record["payment_schedules"]
        .as_array()
        .expect("the record carries one schedule per unit")
        .iter()
        .filter_map(|schedule| schedule["unit"].as_str())
        .collect();
    units.sort_unstable();
    assert_eq!(
        units, UNITS,
        "the normaliser's unit list and the dataset's have drifted"
    );
    assert_eq!(
        record["project"]["name"].as_str(),
        Some(PROJECT_NAME),
        "the project this page offers to answer about is the one in the record",
    );
}

/// Where the fixture recordings are committed, relative to this crate.
const FIXTURE_DIR: &str = "tests/fixtures";

/// The bytes of one committed fixture, read from disk rather than generated.
fn fixture_bytes(name: &str) -> Vec<u8> {
    let path = PathBuf::from(FIXTURE_DIR).join(name);
    std::fs::read(&path).unwrap_or_else(|error| {
        panic!(
            "{} is committed with this crate; run `cargo test -p ask-aunova-simulator \
             the_committed_fixtures` to write it: {error}",
            path.display(),
        )
    })
}

/// Serve one simulator on a loopback port, as the binary serves it.
async fn serve(simulator: Simulator) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a loopback port is free");
    let addr = listener.local_addr().expect("the socket has an address");
    let serving = tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            ask_aunova_simulator::web::app(std::sync::Arc::new(simulator)),
        )
        .await;
    });
    (addr, serving)
}

/// One answer, as much of it as this test will read.
struct Answered {
    /// The HTTP status.
    status: u16,
    /// The body, which every route of this binary answers as UTF-8.
    body: String,
}

impl Answered {
    /// The body as JSON, or a failure naming what came back instead.
    fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body)
            .unwrap_or_else(|error| panic!("the simulator answers JSON ({error}): {}", self.body))
    }
}

/// One blocking HTTP/1.1 GET against a loopback address, bounded.
fn get(addr: SocketAddr, path: &str) -> Answered {
    request(addr, "GET", path, None, &[])
}

/// One blocking HTTP/1.1 POST against a loopback address, bounded.
fn post(addr: SocketAddr, path: &str, content_type: &str, body: &[u8]) -> Answered {
    request(addr, "POST", path, Some(content_type), body)
}

/// One request, written and read by hand.
///
/// Hand-written because that is smaller than it sounds: this posts bytes to a
/// port the test itself started and reads one bounded answer back. Every read
/// is capped, and a response that arrived in a form this cannot read says so
/// rather than being half-parsed.
fn request(
    addr: SocketAddr,
    method: &str,
    path: &str,
    content_type: Option<&str>,
    body: &[u8],
) -> Answered {
    let mut socket = TcpStream::connect(addr).expect("the simulator is listening");
    socket
        .set_read_timeout(Some(Duration::from_secs(30)))
        .expect("a read deadline can be set");

    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
    if let Some(content_type) = content_type {
        head.push_str(&format!(
            "Content-Type: {content_type}\r\nContent-Length: {}\r\n",
            body.len(),
        ));
    }
    head.push_str("\r\n");
    socket
        .write_all(head.as_bytes())
        .expect("the request head is sent");
    socket.write_all(body).expect("the request body is sent");

    let mut response = String::new();
    socket
        .take(MAX_RESPONSE_BYTES)
        .read_to_string(&mut response)
        .expect("the response is UTF-8");
    let (head, body) = response
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("a response has a header block: {response}"));
    assert!(
        !head.to_ascii_lowercase().contains("transfer-encoding"),
        "this client reads a sized body only: {head}",
    );
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("a response starts with a status line: {response}"));
    Answered {
        status,
        body: body.to_owned(),
    }
}
