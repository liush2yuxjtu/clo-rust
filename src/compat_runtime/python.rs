use super::{api, enums, text, DomainCall, ToolResult};
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};

thread_local! {
    static PROCESS: RefCell<Option<Runner>> = const { RefCell::new(None) };
}

const TIME_LIMIT: &str = "CLO Rust Python script exceeded its time limit";

struct Runner {
    child: Child,
    input: ChildStdin,
    replies: Receiver<Result<String, String>>,
}

impl Runner {
    fn new() -> Result<Self, String> {
        let executable = std::env::var_os("CLO_RUST_PYTHON").unwrap_or_else(|| "python3".into());
        let mut child = Command::new(executable)
            .args(["-I", "-u", "-c", include_str!("runner.py")])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("Could not start the CLO Rust Python bridge: {error}"))?;
        let input = child.stdin.take().ok_or("Python stdin unavailable")?;
        let output = child.stdout.take().ok_or("Python stdout unavailable")?;
        let (messages, replies) = mpsc::sync_channel(8);
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                if messages
                    .send(line.map_err(|error| error.to_string()))
                    .is_err()
                {
                    break;
                }
            }
        });
        let mut runner = Self {
            child,
            input,
            replies,
        };
        let catalog = api::catalog(&json!({"limit":500}))?;
        let mut entries = catalog["entries"].clone();
        if let Some(entries) = entries.as_array_mut() {
            for entry in entries {
                let name = entry["name"].as_str().unwrap_or_default();
                if let Some(family) = enums::family(name) {
                    let members = family
                        .members
                        .iter()
                        .map(|(name, value, _)| ((*name).to_owned(), json!(value)))
                        .collect::<serde_json::Map<_, _>>();
                    entry["enum_members"] = Value::Object(members);
                } else if let Some((family, member, _)) = enums::member(name) {
                    entry["enum_type"] = json!(family);
                    entry["enum_member"] = json!(member);
                } else if entry["kind"] == "type" {
                    entry["defaults"] = api::defaults(name)?;
                }
            }
        }
        runner.send(&json!({"entries":entries}))?;
        Ok(runner)
    }

    fn send(&mut self, value: &Value) -> Result<(), String> {
        serde_json::to_writer(&mut self.input, value).map_err(|error| error.to_string())?;
        self.input
            .write_all(b"\n")
            .and_then(|_| self.input.flush())
            .map_err(|error| error.to_string())
    }

    fn request(
        &mut self,
        code: &str,
        timeout: Duration,
        state: &mut Value,
        domain: DomainCall,
    ) -> Result<ToolResult, String> {
        self.send(&json!({"code":code}))?;
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or(TIME_LIMIT)?;
            let line = self
                .replies
                .recv_timeout(remaining)
                .map_err(|error| match error {
                    mpsc::RecvTimeoutError::Timeout => TIME_LIMIT.to_owned(),
                    mpsc::RecvTimeoutError::Disconnected => {
                        "CLO Rust Python process exited during script execution".to_owned()
                    }
                })??;
            let reply: Value = serde_json::from_str(&line)
                .map_err(|error| format!("Invalid Python message: {error}"))?;
            if Instant::now() >= deadline {
                return Err(TIME_LIMIT.into());
            }
            match reply["kind"].as_str() {
                Some("api") => {
                    let name = text(&reply, "name")?;
                    let mut candidate = state.clone();
                    let result = api::invoke(
                        name,
                        &reply["args"],
                        &reply["kwargs"],
                        &mut candidate,
                        domain,
                    );
                    let response = match result {
                        Ok(value) => {
                            *state = candidate;
                            json!({"ok":true,"value":value})
                        }
                        Err(error) => json!({"ok":false,"error":error}),
                    };
                    // Keep completed calls, then stop the interpreter before it runs more code.
                    if Instant::now() >= deadline {
                        return Err(TIME_LIMIT.into());
                    }
                    self.send(&response)?;
                }
                Some("done") => {
                    return Ok(Ok(json!({"value":reply["value"],"stdout":reply["stdout"]})))
                }
                Some("error") => {
                    return Ok(Err(format!(
                        "{}",
                        reply["error"].as_str().unwrap_or("Python script failed")
                    )))
                }
                _ => return Err("Unexpected message from the CLO Rust Python process".into()),
            }
        }
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub(super) fn stop() {
    PROCESS.with_borrow_mut(|runner| *runner = None);
}

pub(super) fn run(args: &Value, state: &mut Value, domain: DomainCall) -> ToolResult {
    let code = text(args, "code")?;
    let seconds = args["timeout_seconds"].as_f64().unwrap_or(600.0);
    if !(0.0 < seconds && seconds <= 86_400.0) {
        return Err("timeout_seconds must be positive and at most 86400".into());
    }
    PROCESS.with_borrow_mut(|process| {
        if process.is_none() {
            *process = Some(Runner::new()?);
        }
        let result = process.as_mut().expect("runner was initialized").request(
            code,
            Duration::from_secs_f64(seconds),
            state,
            domain,
        );
        match result {
            Ok(result) => result,
            Err(error) => {
                *process = None;
                Err(error)
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_api_call_past_deadline_stops_the_script() {
        fn slow_domain(name: &str, _args: &Value, state: &mut Value) -> ToolResult {
            assert_eq!(name, "pattern_create");
            state["patterns"] = json!([{"name":"completed"}]);
            std::thread::sleep(Duration::from_millis(400));
            Ok(json!({"index":0}))
        }

        let mut runner = Runner::new().unwrap();
        let mut scene = json!({"patterns":[]});
        runner
            .request("1", Duration::from_secs(5), &mut scene, slow_domain)
            .unwrap()
            .unwrap();
        let result = runner.request(
            "pattern_api.CreatePatternWithPoints([(0,0),(40,0),(0,40)]); result = 'resumed after deadline'",
            Duration::from_millis(300),
            &mut scene,
            slow_domain,
        );
        assert_eq!(result.unwrap_err(), TIME_LIMIT);
        assert_eq!(scene["patterns"], json!([{"name":"completed"}]));
        assert!(matches!(
            runner.replies.recv_timeout(Duration::from_millis(500)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
    }
}
