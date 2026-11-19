use super::{array, ToolResult};
use serde_json::{json, Value};
use std::io::Write;
use std::process::{Command, Stdio};

pub(super) fn request(name: &str, args: &[Value]) -> ToolResult {
    let url = args[0].as_str().ok_or("REST URL must be a string")?;
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("Use an HTTP or HTTPS URL".into());
    }
    let post = name == "rest_api.CallRESTPost";
    let payload = if post {
        Some(
            args[1]
                .as_str()
                .ok_or("REST request body must be a string")?,
        )
    } else {
        None
    };
    let headers = &args[if post { 2 } else { 1 }];
    let executable = std::env::var_os("CLO_RUST_CURL").unwrap_or_else(|| "curl".into());
    let mut command = Command::new(executable);
    command.args([
        "-q",
        "--silent",
        "--show-error",
        "--include",
        "--http1.1",
        "--location",
        "--max-redirs",
        "10",
        "--connect-timeout",
        "10",
        "--max-time",
        "60",
        "--max-filesize",
        "16777216",
        "--proto",
        "=http,https",
        "--proto-redir",
        "=http,https",
        "--user-agent",
        "Mozilla/4.0",
        "--referer",
        ";auto",
    ]);
    if !headers.is_array() {
        return Err("REST headers must be a list of name/value pairs".into());
    }
    for pair in array(headers) {
        let parts = pair
            .as_array()
            .filter(|parts| parts.len() == 2)
            .ok_or("Each REST header must be a [name, value] pair")?;
        let key = parts[0]
            .as_str()
            .filter(|key| {
                !key.is_empty()
                    && key
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
            })
            .ok_or("Invalid REST header name")?;
        let value = parts[1]
            .as_str()
            .filter(|value| !value.contains(['\r', '\n', '\0']))
            .ok_or("Invalid REST header value")?;
        let header = if value.is_empty() {
            format!("{key};")
        } else {
            format!("{key}: {value}")
        };
        command.arg("--header").arg(header);
    }
    if payload.is_some() {
        command.args(["--data-binary", "@-"]);
    }
    let mut child = command
        .args(["--", url])
        .stdin(if post { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Could not start REST transport: {error}"))?;
    if let Some(payload) = payload {
        let mut input = child.stdin.take().ok_or("REST request stdin unavailable")?;
        let payload = payload.to_owned();
        std::thread::spawn(move || input.write_all(payload.as_bytes()));
    }
    let response = child
        .wait_with_output()
        .map_err(|error| error.to_string())?;
    if !response.status.success() {
        return Err(format!(
            "REST request failed: {}",
            String::from_utf8_lossy(&response.stderr).trim()
        ));
    }
    let response = String::from_utf8_lossy(&response.stdout);
    Ok(json!(separate_body(&response)))
}

fn separate_body(response: &str) -> String {
    let mut offset = 0;
    while let Some(end) = response[offset..].find("\r\n\r\n") {
        offset += end + 4;
        if !response[offset..].starts_with("HTTP/") {
            return format!("{}\r\n\r\n{}", &response[..offset], &response[offset..]);
        }
    }
    response.to_owned()
}
