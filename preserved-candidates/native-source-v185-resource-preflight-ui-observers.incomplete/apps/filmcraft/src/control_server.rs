//! Localhost JSON-lines control server (one request per line, one reply per line). Loopback only.
//!
//! The server only ever reads requests: a line that is not a JSON object with a string `method`
//! (an HTTP request line from a browser's cross-origin `fetch`, a stray `nc`, binary junk) gets
//! one error reply and the connection is closed, so nothing after it is executed. Lines are
//! bounded ([`MAX_LINE`]) and so is the number of open connections ([`MAX_CONNECTIONS`]); junk
//! never reaches the UI thread. See `docs/control-protocol.md`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use filmcraft_engine::AgentOperationControl;
use filmcraft_ui_egui::ControlRequest;
use serde_json::{Value, json};

#[path = "control_source_wire.rs"]
mod source_wire;

/// Longest accepted request line (bytes, without the newline). Requests are small JSON objects;
/// anything longer is refused and the connection closed.
pub const MAX_LINE: usize = 4 * 1024 * 1024;
/// Connections served at once; further ones get an error line and are closed.
pub const MAX_CONNECTIONS: usize = 16;

pub fn start(port: u16, ctx: egui::Context, operations: AgentOperationControl) -> Receiver<ControlRequest> {
    let (tx, rx) = channel::<ControlRequest>();
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("filmcraft: control server failed to bind 127.0.0.1:{port}: {e}");
            return rx;
        }
    };
    eprintln!("filmcraft: control server listening on 127.0.0.1:{port}");
    let started = std::thread::Builder::new().name("control-listener".into()).spawn(move || {
        operations.enable_transport();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| accept_loop(listener, tx, ctx, operations)));
    });
    if let Err(e) = started {
        eprintln!("filmcraft: control listener could not start: {e}");
    }
    rx
}

fn accept_loop(listener: TcpListener, tx: Sender<ControlRequest>, ctx: egui::Context, operations: AgentOperationControl) {
    let open = Arc::new(AtomicUsize::new(0));
    for mut stream in listener.incoming().flatten() {
        if open.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            open.fetch_sub(1, Ordering::SeqCst);
            let _ = writeln!(stream, "{}", json!({"ok": false, "error": "too many control connections"}));
            continue;
        }
        let tx = tx.clone();
        let ctx = ctx.clone();
        let operations = operations.clone();
        // The slot is given back when the guard drops: after `serve` returns, if it panics, or if
        // the thread can't be started (the closure, and with it the guard, is dropped).
        let slot = Slot(Arc::clone(&open));
        let spawned = std::thread::Builder::new().name("control-conn".into()).spawn(move || {
            let _slot = slot;
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| serve(stream, &tx, &ctx, &operations)));
        });
        if let Err(e) = spawned {
            eprintln!("filmcraft: control server could not start a connection thread: {e}");
        }
    }
}

/// One counted connection; decrements the open-connection count when dropped.
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

fn serve(stream: TcpStream, tx: &Sender<ControlRequest>, ctx: &egui::Context, operations: &AgentOperationControl) {
    let Ok(read) = stream.try_clone() else { return };
    serve_lines(read, stream, |method, params| {
        if method == "engine.cancelSaveOperationWait" {
            return Some(match operations.cancel_save_wait(&params) {
                Ok(result) => json!({"ok":true,"result":result}),
                Err(error) => json!({"ok":false,"error":error.to_string()}),
            });
        }
        if method == "engine.cancelOperation" {
            return Some(match operations.cancel(&params) {
                Ok(result) => json!({"ok":true,"result":result}),
                Err(error) => json!({"ok":false,"error":error.to_string()}),
            });
        }
        let wait = Duration::from_millis(filmcraft_engine::session_worker::control_reply_wait_ms(&method, &params));
        let (req, rrx) = ControlRequest::new(method, params);
        tx.send(req).ok()?;
        ctx.request_repaint();
        Some(wait_for_reply(&rrx, wait))
    });
}

fn wait_for_reply(reply: &std::sync::mpsc::Receiver<Value>, timeout: Duration) -> Value {
    reply.recv_timeout(timeout).unwrap_or_else(|_| {
        // The request was forwarded. Its worker may still commit or may have
        // committed before the reply disappeared; timeout is never a rejection.
        json!({"ok":false,"error":"native reply unavailable; reconcile the original request before another edit","outcome":"unknown"})
    })
}

/// One parsed line.
enum Line {
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Blank,
    /// Not a request: reply with this error and close the connection.
    Reject(String),
}

fn parse_line(bytes: &[u8]) -> Line {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Line::Reject("not a control request (invalid UTF-8); closing the connection".into());
    };
    let text = text.trim();
    if text.is_empty() {
        return Line::Blank;
    }
    let msg = match serde_json::from_str::<Value>(text) {
        Ok(v) => v,
        Err(e) => return Line::Reject(format!("bad JSON ({e}); expected one JSON request object per line; closing the connection")),
    };
    let Some(obj) = msg.as_object() else {
        return Line::Reject("not a control request (expected a JSON object); closing the connection".into());
    };
    let Some(method) = obj.get("method").and_then(Value::as_str) else {
        return Line::Reject("not a control request (missing string `method`); closing the connection".into());
    };
    Line::Request { id: obj.get("id").cloned().unwrap_or(Value::Null), method: method.to_string(), params: obj.get("params").cloned().unwrap_or(json!({})) }
}

/// Read one `\n`-terminated line of at most `max` bytes into `buf` (newline stripped).
/// `Ok(false)` at end of stream; `Err` when the line is too long or the read fails.
fn read_bounded_line(reader: &mut impl BufRead, buf: &mut Vec<u8>, max: usize) -> Result<bool, String> {
    buf.clear();
    // `take` bounds the bytes buffered for one line: a peer can't make us allocate more than `max`.
    let limit = u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1);
    let n = reader.take(limit).read_until(b'\n', buf).map_err(|e| e.to_string())?;
    if n == 0 {
        return Ok(false);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
        return Ok(true);
    }
    if buf.len() > max {
        return Err(format!("request line longer than {max} bytes; closing the connection"));
    }
    // Last line without a newline, then end of stream.
    Ok(true)
}

/// Serve requests from `read`, answering on `out`, until the peer closes, sends something that is
/// not a request, or `handle` returns `None` (app gone). `handle` gets `(method, params)` and
/// returns the reply object (the request's `id` is added here).
fn serve_lines<R: Read, W: Write>(read: R, mut out: W, mut handle: impl FnMut(String, Value) -> Option<Value>) {
    let mut reader = BufReader::new(read);
    let mut buf = Vec::new();
    loop {
        let reject = match read_bounded_line(&mut reader, &mut buf, MAX_LINE) {
            Ok(false) => return,
            Err(e) => e,
            Ok(true) => match parse_line(&buf) {
                Line::Blank => continue,
                Line::Reject(e) => e,
                Line::Request { id, method, mut params } => {
                    let encoding = source_wire::take_request(&method, &mut params);
                    let (mut r, exact) = match encoding {
                        Err(error) => (json!({"ok":false,"error":error,"outcome":"rejected"}), false),
                        Ok(exact) => {
                            let Some(reply) = handle(method, params) else { return };
                            (reply, exact)
                        }
                    };
                    if let Some(o) = r.as_object_mut() {
                        o.insert("id".into(), id.clone());
                    }
                    if exact {
                        // The complete bound includes the actual original request ID.
                        r = source_wire::project(r);
                        if let Some(o) = r.as_object_mut() {
                            // Projection failure still answers the same request.
                            o.insert("id".into(), id);
                        }
                    }
                    if writeln!(out, "{r}").and_then(|()| out.flush()).is_err() {
                        return;
                    }
                    continue;
                }
            },
        };
        let _ = writeln!(out, "{}", json!({"ok": false, "error": reject})).and_then(|()| out.flush());
        return;
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn forwarded_timeout_and_lost_reply_never_claim_an_edit_was_rejected() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let timed_out = super::wait_for_reply(&receiver, std::time::Duration::ZERO);
        assert_eq!(timed_out["outcome"], "unknown");
        sender.send(serde_json::json!({"ok":true,"result":{"status":"committed"}})).unwrap();
        assert_eq!(super::wait_for_reply(&receiver, std::time::Duration::ZERO)["result"]["status"], "committed");
        drop(sender);
        assert_eq!(super::wait_for_reply(&receiver, std::time::Duration::ZERO)["outcome"], "unknown");
    }
    use super::*;

    #[test]
    fn negotiated_source_responses_use_original_parser_and_serialize_all_nested_clocks() {
        let mut input = Vec::new();
        let commands = [
            "agent.readMulticamInputs",
            "agent.snapshotStatus",
            "agent.cancelSnapshot",
            "agent.prepareMulticamProduction",
            "agent.readMulticamProductionPlan",
            "agent.apply",
            "agent.receipt",
            "agent.readHistory",
            "agent.undo",
            "agent.redo",
        ];
        for (i, command) in commands.iter().enumerate() {
            writeln!(
                input,
                "{}",
                json!({"id":i,"method":"engine.execute","params":{
                "command":command,"sourceIntegerEncoding":source_wire::ENCODING,
                "params":{"token":"original-retained-token","offset":i64::MIN}}})
            )
            .unwrap();
        }
        let mut output = Vec::new();
        let mut calls = 0;
        serve_lines(input.as_slice(), &mut output, |method, params| {
            assert_eq!(method, "engine.execute");
            assert!(params.get("sourceIntegerEncoding").is_none());
            assert_eq!(params["params"]["offset"], i64::MIN);
            calls += 1;
            // The same path handles immediate, snapshot completion and waited worker envelopes.
            Some(json!({"ok":true,"result":{"status":"complete","result":{
                "kind":"native_multicam_production_inputs","targets":{"instances":[
                    {"clip":{"source_in":i64::MIN,"start":i64::MAX,"markers":[{"time":9_007_199_254_740_993i64}]}}]},
                "report":{"lag":-9_007_199_254_740_993i64},"opaque":"{\"tick\":9223372036854775807}"}}}))
        });
        assert_eq!(calls, commands.len());
        for (i, line) in String::from_utf8(output).unwrap().lines().enumerate() {
            let reply: Value = serde_json::from_str(line).unwrap();
            assert_eq!(reply["id"], i);
            assert_eq!(reply["sourceIntegerEncoding"], source_wire::ENCODING);
            assert_eq!(reply["result"]["result"]["targets"]["instances"][0]["clip"]["start"], i64::MAX.to_string());
            assert_eq!(reply["result"]["result"]["report"]["lag"], "-9007199254740993");
            assert_eq!(reply["result"]["result"]["opaque"], "{\"tick\":9223372036854775807}");
        }
    }

    #[test]
    fn invalid_negotiation_never_dispatches_and_legacy_replies_remain_native_numbers() {
        let mut input = Vec::new();
        for params in [
            json!({"command":"file.newProject","sourceIntegerEncoding":source_wire::ENCODING}),
            json!({"command":"agent.apply","sourceIntegerEncoding":"future"}),
            json!({"command":"agent.apply","sourceIntegerEncoding":false}),
            json!({"command":"agent.readMulticamInputs"}),
        ] {
            writeln!(input, "{}", json!({"id":1,"method":"engine.execute","params":params})).unwrap();
        }
        let mut output = Vec::new();
        let mut calls = 0;
        serve_lines(input.as_slice(), &mut output, |_, _| {
            calls += 1;
            Some(json!({"ok":true,"result":{"time":i64::MAX}}))
        });
        assert_eq!(calls, 1);
        let replies = String::from_utf8(output).unwrap().lines().map(|s| serde_json::from_str::<Value>(s).unwrap()).collect::<Vec<_>>();
        assert_eq!(replies.len(), 4);
        for reply in &replies[..3] {
            assert_eq!(reply["ok"], false);
            assert_eq!(reply["outcome"], "rejected");
        }
        assert_eq!(replies[3]["result"]["time"], i64::MAX);
        assert!(replies[3].get("sourceIntegerEncoding").is_none());
    }

    #[test]
    fn negotiated_waited_worker_loss_and_projection_failure_keep_unknown_outcomes() {
        for reply in [
            json!({"ok":false,"error":"native reply unavailable","outcome":"unknown"}),
            json!({"ok":true}),
            json!({"ok":true,"result":"x".repeat(source_wire::MAX_REPLY_BYTES)}),
        ] {
            let input = json!({"id":"original-request","method":"engine.execute","params":{
                "command":"agent.apply","params":{"token":"original"},"sourceIntegerEncoding":source_wire::ENCODING}})
            .to_string()
                + "\n";
            let mut output = Vec::new();
            serve_lines(input.as_bytes(), &mut output, |_, _| {
                let (sender, receiver) = std::sync::mpsc::channel();
                sender.send(reply.clone()).unwrap();
                Some(wait_for_reply(&receiver, std::time::Duration::from_millis(10)))
            });
            let result: Value = serde_json::from_slice(&output).unwrap();
            assert_eq!(result["id"], "original-request");
            assert_eq!(result["outcome"], "unknown");
            assert_eq!(result["sourceIntegerEncoding"], source_wire::ENCODING);
            assert!(result.get("result").is_none());
        }
    }

    #[test]
    fn complete_exact_reply_bound_includes_request_id_and_failure_retains_that_id() {
        let id = "original-request-".repeat(512);
        let request = json!({"id":id,"method":"engine.execute","params":{"command":"agent.receipt",
            "params":{"token":"original"},"sourceIntegerEncoding":source_wire::ENCODING}})
        .to_string()
            + "\n";
        let mut output = Vec::new();
        serve_lines(request.as_bytes(), &mut output, |_, _| Some(json!({"ok":true,"result":"x".repeat(source_wire::MAX_REPLY_BYTES-128)})));
        let reply: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(reply["id"], id);
        assert_eq!(reply["outcome"], "unknown");
        assert!(reply.get("result").is_none());
        assert!(output.len() < source_wire::MAX_REPLY_BYTES);
    }
    #[test]
    fn original_parser_dispatches_exact_string_offset_once_and_invalid_offsets_never_dispatch() {
        for (text, expected) in [
            ("-9223372036854775808", Some(i64::MIN)),
            ("9223372036854775807", Some(i64::MAX)),
            ("9007199254740993", Some(9_007_199_254_740_993)),
            ("-0", None),
            ("9223372036854775808", None),
            ("01", None),
        ] {
            let input = json!({"id":1,"method":"engine.execute","params":{
                "command":"agent.prepareMulticamProduction","sourceIntegerEncoding":source_wire::ENCODING,
                "params":{"edit":{"gesture":"synchronize","options":{"offset":text}}}}})
            .to_string()
                + "\n";
            let mut output = Vec::new();
            let mut count = 0;
            serve_lines(input.as_bytes(), &mut output, |_, params| {
                count += 1;
                assert_eq!(params["params"]["edit"]["options"]["offset"].as_i64(), expected);
                Some(json!({"ok":true,"result":{"offset":params["params"]["edit"]["options"]["offset"]}}))
            });
            let reply: Value = serde_json::from_slice(&output).unwrap();
            assert_eq!(count, usize::from(expected.is_some()));
            if expected.is_some() { assert_eq!(reply["result"]["offset"], text) } else { assert_eq!(reply["outcome"], "rejected") }
        }
    }

    fn run(input: &[u8]) -> (Vec<Value>, Vec<String>) {
        let mut out = Vec::new();
        let mut calls = Vec::new();
        serve_lines(input, &mut out, |m, _p| {
            calls.push(m);
            Some(json!({"ok": true, "result": null}))
        });
        let replies = String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str::<Value>(l).unwrap()).collect();
        (replies, calls)
    }

    #[test]
    fn requests_are_answered_in_order_with_ids() {
        let (r, calls) = run(b"{\"id\":1,\"method\":\"a\"}\n\n{\"id\":2,\"method\":\"b\",\"params\":{}}\r\n{\"method\":\"c\"}");
        assert_eq!(calls, ["a", "b", "c"]);
        assert_eq!(r.len(), 3);
        assert_eq!(r[0]["id"], 1);
        assert_eq!(r[1]["id"], 2);
        assert_eq!(r[2]["id"], Value::Null);
    }

    #[test]
    fn http_request_closes_before_the_body_runs() {
        // A browser's cross-origin `fetch` POST (issue #119): the request line is rejected and the
        // connection closed, so the JSON body is never executed.
        let http = b"POST / HTTP/1.1\r\nHost: 127.0.0.1:9876\r\nOrigin: https://attacker.example\r\nContent-Type: text/plain\r\n\r\n\
            {\"id\":7,\"method\":\"engine.execute\",\"params\":{\"command\":\"file.openDemoProject\",\"params\":{}}}\n";
        let (r, calls) = run(http);
        assert!(calls.is_empty(), "nothing may run: {calls:?}");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0]["ok"], false);
        assert!(r[0]["error"].as_str().unwrap().contains("closing the connection"));
    }

    #[test]
    fn non_request_json_closes_the_connection() {
        for junk in [
            &b"[1,2]\n{\"method\":\"a\"}\n"[..],
            b"{\"id\":1}\n{\"method\":\"a\"}\n",
            b"{\"method\":5}\n{\"method\":\"a\"}\n",
            b"\xff\xfe\n{\"method\":\"a\"}\n",
        ] {
            let (r, calls) = run(junk);
            assert!(calls.is_empty(), "{calls:?}");
            assert_eq!(r.len(), 1);
            assert_eq!(r[0]["ok"], false);
        }
    }

    #[test]
    fn a_request_before_junk_still_runs() {
        let (r, calls) = run(b"{\"method\":\"a\"}\nGET / HTTP/1.1\n{\"method\":\"b\"}\n");
        assert_eq!(calls, ["a"]);
        assert_eq!(r.len(), 2);
        assert_eq!(r[1]["ok"], false);
    }

    #[test]
    fn overlong_line_is_refused() {
        // One byte over the limit, then a valid request: neither runs.
        let mut input = vec![b'x'; MAX_LINE + 1];
        input.extend_from_slice(b"\n{\"method\":\"a\"}\n");
        let (r, calls) = run(&input);
        assert!(calls.is_empty(), "{calls:?}");
        assert_eq!(r.len(), 1);
        assert!(r[0]["error"].as_str().unwrap().contains("longer than"));
    }

    #[test]
    fn line_at_the_limit_is_accepted() {
        let mut buf = Vec::new();
        let line = vec![b'a'; 10];
        let mut input = line.clone();
        input.push(b'\n');
        assert_eq!(read_bounded_line(&mut &input[..], &mut buf, 10), Ok(true));
        assert_eq!(buf, line);
        assert!(read_bounded_line(&mut &[b'a'; 11][..], &mut buf, 10).is_err());
    }

    #[test]
    fn tcp_connection_is_closed_after_junk() {
        // End to end over a real loopback socket (the transport the app uses).
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            let read = s.try_clone().unwrap();
            serve_lines(read, s, |_, _| Some(json!({"ok": true})));
        });
        let mut c = TcpStream::connect(addr).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        c.write_all(b"{\"id\":7,\"method\":\"x\"}\nhello\n{\"method\":\"y\"}\n").unwrap();
        let mut text = String::new();
        c.read_to_string(&mut text).unwrap(); // returns at EOF: the server closed the connection
        server.join().unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "{text}");
        assert!(lines[0].contains("\"id\":7"));
        assert!(lines[1].contains("closing the connection"));
    }

    #[test]
    fn extra_connections_are_refused() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, _rx) = channel::<ControlRequest>();
        std::thread::spawn(move || accept_loop(listener, tx, egui::Context::default(), AgentOperationControl::default()));
        // Idle connections are accepted in order and held open.
        let held: Vec<TcpStream> = (0..16).map(|_| TcpStream::connect(addr).unwrap()).collect();
        let mut extra = TcpStream::connect(addr).unwrap();
        extra.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut line = String::new();
        BufReader::new(&mut extra).read_line(&mut line).unwrap();
        assert!(line.contains("too many control connections"), "{line}");
        drop(held);
    }

    #[test]
    fn cancellation_bypasses_a_queued_ui_write_and_prevents_its_late_publication() {
        use filmcraft_engine::Session;
        let mut session = Session::default();
        session.execute("file.openDemoProject", json!({})).unwrap();
        let token = "independent-cancel-ticket-000000000000000001";
        let doc = session.execute("agent.snapshot", json!({"token":token})).unwrap();
        let revision = session.revision;
        let project = session.project.clone();
        let operations = session.agent_operation_control();
        operations.enable_transport();
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let (tx, rx) = channel::<ControlRequest>();
        let server = std::thread::spawn(move || {
            let mut handlers = Vec::new();
            for _ in 0..2 {
                let (stream, _) = listener.accept().unwrap();
                let tx = tx.clone();
                let control = operations.clone();
                handlers.push(std::thread::spawn(move || serve(stream, &tx, &egui::Context::default(), &control)));
            }
            for h in handlers {
                h.join().unwrap();
            }
        });
        let clip = doc["sequence"]["video"][0]["items"][0]["clip"].clone();
        let writer = std::thread::spawn(move || {
            let mut s = TcpStream::connect(address).unwrap();
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            writeln!(s,"{}",json!({"id":1,"method":"engine.execute","params":{"command":"agent.apply","params":{"token":token,"label":"Generated queued edit","operations":[{"command":"timeline.razor","params":{"clip":clip,"time":1}}]}}})).unwrap();
            let mut line = String::new();
            BufReader::new(s).read_line(&mut line).unwrap();
            serde_json::from_str::<Value>(&line).unwrap()
        });
        let queued = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let mut cancel = TcpStream::connect(address).unwrap();
        cancel.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        writeln!(cancel, "{}", json!({"id":2,"method":"engine.cancelOperation","params":{"token":token}})).unwrap();
        let mut line = String::new();
        BufReader::new(cancel).read_line(&mut line).unwrap();
        let ack: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(ack["id"], 2);
        assert_eq!(ack["result"]["cancellationAccepted"], true);
        assert!(rx.try_recv().is_err(), "Cancel never entered the blocked UI queue");
        let result = session.execute(queued.params["command"].as_str().unwrap(), queued.params["params"].clone());
        assert!(result.as_ref().unwrap_err().to_string().contains("cancelled"));
        queued.reply.send(json!({"ok":false,"error":result.unwrap_err().to_string()})).unwrap();
        assert_eq!(writer.join().unwrap()["ok"], false);
        server.join().unwrap();
        assert_eq!(session.revision, revision);
        assert!(Arc::ptr_eq(&session.project, &project));
    }
    #[test]
    fn save_wait_stop_bypasses_blocked_ui_over_tcp_and_keeps_the_same_native_owner_until_exact_cancel() {
        use filmcraft_engine::{
            Session,
            session_worker::{Completion, Pending, SessionWorker},
        };
        use std::sync::{Condvar, Mutex};
        use std::time::{Instant, SystemTime, UNIX_EPOCH};
        fn finish(work: &mut Pending) -> Completion {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if let Some(done) = work.try_finish() {
                    return done;
                }
                assert!(Instant::now() < deadline, "same original native worker did not return");
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        struct Release(Arc<(Mutex<bool>, Condvar)>);
        impl Drop for Release {
            fn drop(&mut self) {
                *self.0.0.lock().unwrap_or_else(|e| e.into_inner()) = true;
                self.0.1.notify_all();
            }
        }
        let root =
            std::env::temp_dir().join(format!("save-socket-{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir(&root).unwrap();
        let root = std::fs::canonicalize(root).unwrap();
        let project_path = root.join("Original.fcproj");
        let mut session = Session::default();
        let scope = json!({"projectId":"p_save_socket","workspaceId":"6b742be6-cddd-40e0-93b2-857aabc9a4e9"});
        session.execute("file.newProject", json!({"name":"Generated Save socket","workspace":scope})).unwrap();
        session.execute("file.saveAs", json!({"path":project_path})).unwrap();
        session.start_autosave(filmcraft_engine::autosave::AutosaveConfig::new(root.join("runtime"))).unwrap();
        session.sync_persistence();
        let worker = SessionWorker::new().unwrap();
        worker.install(&mut session).unwrap();
        let operations = session.agent_operation_control();
        operations.enable_transport();
        let token = "save-socket-read-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let job = "save-socket-job-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let operation = "save-socket-op-cccccccccccccccccccccccccccccccc";
        let doc = session.execute("agent.readProject", json!({"token":token,"scope":scope,"projectPath":project_path})).unwrap();
        session
            .execute(
                "agent.startProjectSavePlan",
                json!({"job":job,"query":{"token":token,"scope":scope,
            "beforeProjectSha256":doc["projectState"]["sha256"],"intent":{"kind":"save_copy","path":root.join("Copy.fcproj")}}}),
            )
            .unwrap();
        let delivery_deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let ready = session.execute("agent.projectSavePlanStatus", json!({"job":job})).unwrap();
            if ready["status"] == "delivered" {
                break;
            }
            assert_eq!(ready["status"], "running");
            assert!(Instant::now() < delivery_deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        let request = json!({"job":job,"operation":operation});
        let project = session.project.clone();
        let revision = session.revision;
        let bytes = std::fs::read(&project_path).unwrap();
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let _release_on_failure = Release(gate.clone());
        let held = gate.clone();
        let (entered, reached) = channel();
        let mut beginning = worker
            .start(
                &mut session,
                "agent.beginProjectSaveOperation",
                request.clone(),
                Some(Arc::new(move || {
                    entered.send(()).unwrap();
                    let guard = held.0.lock().unwrap();
                    let (open, timed) = held.1.wait_timeout_while(guard, Duration::from_secs(10), |open| !*open).unwrap();
                    assert!(*open && !timed.timed_out(), "completion gate was not released");
                })),
            )
            .unwrap();
        reached.recv_timeout(Duration::from_secs(5)).unwrap();
        let begun = finish(&mut beginning);
        session = begun.session;
        assert_eq!(begun.reply.result.unwrap()["status"], "held");
        assert_eq!(session.pending_save_operation_request(), Some(request.clone()));
        // The real worker is held in its existing completion callback. The
        // next real status task is deterministically queued with the same key.
        let mut waiting = worker.start(&mut session, "agent.projectSaveOperationStatus", request.clone(), None).unwrap();
        assert_eq!(waiting.progress()["phase"], "queued");
        assert!(session.is_display_only());
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let (tx, rx) = channel::<ControlRequest>();
        let transport = operations.clone();
        let server = std::thread::spawn(move || {
            let mut handlers = Vec::new();
            for _ in 0..2 {
                let (stream, _) = listener.accept().unwrap();
                let tx = tx.clone();
                let control = transport.clone();
                handlers.push(std::thread::spawn(move || serve(stream, &tx, &egui::Context::default(), &control)));
            }
            for handler in handlers {
                handler.join().unwrap();
            }
        });
        let cancelling = request.clone();
        let cancellation_reply = std::thread::spawn(move || {
            let mut stream = TcpStream::connect(address).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            writeln!(
                stream,
                "{}",
                json!({"id":1,"method":"engine.execute","params":{
                "command":"agent.cancelProjectSaveOperation","params":cancelling}})
            )
            .unwrap();
            let mut line = String::new();
            BufReader::new(stream).read_line(&mut line).unwrap();
            serde_json::from_str::<Value>(&line).unwrap()
        });
        let queued_cancel = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let stream = TcpStream::connect(address).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut stop = BufReader::new(stream);
        let mut rpc = |id: u64, params: Value| -> Value {
            writeln!(stop.get_mut(), "{}", json!({"id":id,"method":"engine.cancelSaveOperationWait","params":params})).unwrap();
            let mut line = String::new();
            stop.read_line(&mut line).unwrap();
            let reply: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(reply["id"], id);
            reply
        };
        let wrong = rpc(2, json!({"job":job,"operation":"save-socket-foreign-dddddddddddddddddddddddddddddddd"}));
        assert_eq!(wrong["result"]["waitStopAccepted"], false);
        assert_eq!(waiting.progress()["phase"], "queued");
        assert_eq!(rpc(3, json!({"job":job}))["ok"], false);
        let stopped = rpc(4, request.clone());
        assert_eq!(stopped["ok"], true);
        assert_eq!(stopped["result"]["waitStopAccepted"], true);
        assert_eq!(stopped["result"]["job"], job);
        assert_eq!(stopped["result"]["operation"], operation);
        assert_eq!(stopped["result"]["nativeOperationCancelled"], false);
        assert_eq!(stopped["result"]["nativeComplete"], false);
        assert_eq!(stopped["result"]["writeAuthority"], false);
        assert_eq!(waiting.progress()["phase"], "stopping");
        assert!(rx.try_recv().is_err(), "wait Stop never entered the blocked UI queue");
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        let stopped_status = finish(&mut waiting);
        session = stopped_status.session;
        assert!(stopped_status.reply.result.is_err());
        assert_eq!(session.pending_save_operation_request(), Some(request.clone()));
        assert!(Arc::ptr_eq(&session.project, &project));
        assert_eq!(session.revision, revision);
        assert_eq!(std::fs::read(&project_path).unwrap(), bytes);
        assert!(!root.join("Copy.fcproj").exists());
        // Only the exact original UI cancellation, after the actual Session
        // returns, can acknowledge native cancellation and release its owner.
        let mut cancel = worker.start(&mut session, queued_cancel.params["command"].as_str().unwrap(), queued_cancel.params["params"].clone(), None).unwrap();
        let cancelled = finish(&mut cancel);
        session = cancelled.session;
        assert_eq!(cancelled.reply.result.as_ref().unwrap()["status"], "cancelled");
        queued_cancel.reply.send(cancelled.reply.envelope()).unwrap();
        assert_eq!(cancellation_reply.join().unwrap()["result"]["status"], "cancelled");
        assert!(session.pending_save_operation_request().is_none());
        assert_eq!(rpc(5, request)["result"]["waitStopAccepted"], false);
        assert!(rx.try_recv().is_err());
        drop(stop);
        server.join().unwrap();
        assert!(Arc::ptr_eq(&session.project, &project));
        assert_eq!(session.revision, revision);
        assert_eq!(std::fs::read(&project_path).unwrap(), bytes);
        assert!(!root.join("Copy.fcproj").exists());
        session.shutdown();
    }
}
