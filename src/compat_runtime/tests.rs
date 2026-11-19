use super::*;

fn domain(name: &str, args: &Value, state: &mut Value) -> ToolResult {
    match name {
        "avatar_measurements" => Ok(json!({"body":state["avatar"]["body"]})),
        "pattern_create" => {
            let patterns = state["patterns"]
                .as_array_mut()
                .ok_or("test scene has no patterns")?;
            let index = patterns.len();
            patterns.push(
                json!({"name":args["name"].as_str().unwrap_or("Unnamed"),"points":args["points"]}),
            );
            Ok(json!({"index":index}))
        }
        "pattern_info" => {
            let index = args["pattern"]
                .as_str()
                .unwrap_or_default()
                .parse::<usize>()
                .map_err(|error| error.to_string())?;
            state["patterns"]
                .get(index)
                .cloned()
                .ok_or_else(|| "Unknown pattern".into())
        }
        "__clo_pattern_outline" => {
            let index = args["pattern"]
                .as_str()
                .unwrap_or_default()
                .parse::<usize>()
                .map_err(|error| error.to_string())?;
            let pattern = state["patterns"].get(index).ok_or("Unknown pattern")?;
            let offset_x = pattern["pos_2d"][0].as_f64().unwrap_or_default();
            let offset_y = pattern["pos_2d"][1].as_f64().unwrap_or_default();
            let outline = array(&pattern["points"])
                .iter()
                .map(|point| {
                    json!([
                        point[0].as_f64().unwrap_or_default() + offset_x,
                        point[1].as_f64().unwrap_or_default() + offset_y
                    ])
                })
                .collect::<Vec<_>>();
            Ok(json!({"points":outline}))
        }
        "pattern_ops" => {
            let index = args["pattern"]
                .as_str()
                .unwrap_or_default()
                .parse::<usize>()
                .map_err(|error| error.to_string())?;
            if args["op"] == "move_2d" {
                let x = state["patterns"][index]["pos_2d"][0]
                    .as_f64()
                    .unwrap_or_default();
                let y = state["patterns"][index]["pos_2d"][1]
                    .as_f64()
                    .unwrap_or_default();
                state["patterns"][index]["pos_2d"] = json!([
                    x + args["args"]["dx"].as_f64().unwrap(),
                    y + args["args"]["dy"].as_f64().unwrap()
                ]);
            } else {
                state["patterns"][index]["name"] = args["args"]["name"].clone();
            }
            Ok(json!({"ok":true}))
        }
        "pattern_delete" => {
            state["patterns"] = json!([]);
            Ok(json!({"deleted":1}))
        }
        name => Err(format!("Test domain has no implementation for {name}")),
    }
}

fn tool(name: &str, args: Value, state: &mut Value) -> ToolResult {
    dispatch(name, &args, state, domain).ok_or("Unknown tool")?
}

#[test]
fn catalog_describe_search_and_bind() {
    let mut scene = json!({"patterns":[]});
    let first = tool(
        "clo_api_catalog",
        json!({"module":"pattern_api","limit":5}),
        &mut scene,
    )
    .unwrap();
    assert_eq!(first["entries"].as_array().unwrap().len(), 5);
    assert_eq!(first["next_offset"], 5);
    let first_name = first["entries"][0]["name"].as_str().unwrap();
    let evidence = tool("clo_api_describe", json!({"name":first_name}), &mut scene).unwrap();
    assert_eq!(evidence["name"], first_name);
    let description = tool(
        "clo_api_doc",
        json!({"function_name":"GetPatternCount"}),
        &mut scene,
    )
    .unwrap();
    assert!(description
        .as_str()
        .unwrap()
        .contains("pattern_api.GetPatternCount"));
    let summary = tool(
        "clo_api_search",
        json!({"query":"GetPatternCount"}),
        &mut scene,
    )
    .unwrap();
    let summary: Value = serde_json::from_str(summary.as_str().unwrap()).unwrap();
    assert_eq!(summary["total"], 1);
    let error = tool(
        "clo_api_call",
        json!({"name":"pattern_api.GetPatternCount","kwargs":{"extra":1}}),
        &mut scene,
    )
    .unwrap_err();
    assert!(error.contains("Unexpected API argument"));
}

#[test]
fn handles_and_bridge_lifecycle() {
    let mut scene = json!({"patterns":[]});
    let handle = tool(
        "clo_api_call",
        json!({"name":"ApiTypes.ImportExportOption"}),
        &mut scene,
    )
    .unwrap();
    let id = handle["value"]["$handle"].as_str().unwrap();
    let released = tool("clo_api_release", json!({"handles":[id,id]}), &mut scene).unwrap();
    assert_eq!(released["released"], 1);
    assert_eq!(
        tool("clo_status", json!({}), &mut scene).unwrap()["bridge"],
        "up"
    );
    tool("clo_stop_bridge", json!({}), &mut scene).unwrap();
    assert_eq!(
        tool("clo_status", json!({}), &mut scene).unwrap()["bridge"],
        "down"
    );
    assert!(tool("clo_api_catalog", json!({}), &mut scene).is_err());
    tool("clo_launch", json!({"with_bridge":true}), &mut scene).unwrap();
    assert_eq!(
        tool("clo_status", json!({}), &mut scene).unwrap()["bridge"],
        "up"
    );
    assert!(tool("clo_control_guide", json!({}), &mut scene).unwrap()["route"].is_array());
}

#[test]
fn python_runs_real_code_and_mutates_live_scene() {
    let mut scene = json!({"patterns":[]});
    let code = "created = pattern_api.CreatePatternWithPoints([(0,0), (2,0), (2,2)]); print('created', created); result = pattern_api.GetPatternCount()";
    let result = tool("clo_run_python", json!({"code":code}), &mut scene).unwrap();
    assert_eq!(result, json!({"value":1,"stdout":"created 0\n"}));
    let name = tool(
        "clo_api_call",
        json!({"name":"pattern_api.GetPatternPieceName","kwargs":{"index":0}}),
        &mut scene,
    )
    .unwrap();
    assert_eq!(name["value"], "Unnamed");
    let second = tool(
        "clo_run_python",
        json!({"code":"created + pattern_api.GetPatternCount()"}),
        &mut scene,
    )
    .unwrap();
    assert_eq!(second["value"], 1);
    let error = tool(
        "clo_run_python",
        json!({"code":"while True:\n pass","timeout_seconds":0.2}),
        &mut scene,
    )
    .unwrap_err();
    assert!(error.contains("time limit"));
    let after_timeout = tool(
        "clo_run_python",
        json!({"code":"pattern_api.GetPatternCount()"}),
        &mut scene,
    )
    .unwrap();
    assert_eq!(after_timeout["value"], 1);
    tool("clo_stop_bridge", json!({}), &mut scene).unwrap();
}

#[test]
fn guarded_workbench_events_edit_scene() {
    let mut scene = json!({"patterns":[]});
    let initial = tool("clo_ui_state", json!({}), &mut scene).unwrap();
    let args = json!({"snapshot":initial["snapshot"],"action":"press","element":2});
    let updated = tool("clo_ui_action", args.clone(), &mut scene).unwrap();
    assert_eq!(scene["patterns"].as_array().unwrap().len(), 1);
    assert!(tool("clo_ui_action", args, &mut scene)
        .unwrap_err()
        .contains("Stale UI snapshot"));
    let rename = json!({"snapshot":updated["state"]["snapshot"],"action":"set_value","element":8,"value":"Front"});
    let renamed = tool("clo_ui_action", rename, &mut scene).unwrap();
    assert_eq!(scene["patterns"][0]["name"], "Front");
    assert!(renamed["state"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|node| node["AXTitle"] == "Front"));
}

#[test]
fn workbench_screenshot_has_png_pixels() {
    if std::env::var_os("CLO_RUST_TEST_BROWSER").is_none() {
        return;
    }
    let mut scene = json!({"patterns":[{"name":"Front","points":[[0,0],[50,0],[50,100],[0,100]]}]});
    let result = tool("clo_ui_screenshot", json!({}), &mut scene).unwrap();
    let path = result["images"][0]["path"].as_str().unwrap();
    let png = std::fs::read(path).unwrap();
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), 1024);
    assert_eq!(u32::from_be_bytes(png[20..24].try_into().unwrap()), 720);
    let directory = std::path::Path::new(path).parent().unwrap();
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn geometry_queries_and_pointer_drag() {
    let mut scene = json!({"patterns":[{"name":"Front","points":[[0,0],[50,0],[50,100],[0,100]]}]});
    let area = tool(
        "clo_api_call",
        json!({"name":"pattern_api.GetPatternPieceArea","args":[0]}),
        &mut scene,
    )
    .unwrap();
    assert_eq!(area["value"], 5000.0);
    let hit = tool(
        "clo_api_call",
        json!({"name":"pattern_api.GetPatternIndexFrom2DView","args":[644,415]}),
        &mut scene,
    )
    .unwrap();
    assert_eq!(hit["value"], 0);
    let initial = tool("clo_ui_state", json!({}), &mut scene).unwrap();
    let action = json!({"snapshot":initial["snapshot"],"action":"drag","x":644,"y":415,"end_x":844,"end_y":415});
    tool("clo_ui_action", action, &mut scene).unwrap();
    assert_eq!(scene["patterns"][0]["pos_2d"], json!([80.0, 0.0]));
    let empty = tool(
        "clo_api_call",
        json!({"name":"pattern_api.GetPatternIndexFrom2DView","args":[644,415]}),
        &mut scene,
    )
    .unwrap();
    assert_eq!(empty["value"], -1);
    let moved = tool(
        "clo_api_call",
        json!({"name":"pattern_api.GetPatternIndexFrom2DView","args":[844,415]}),
        &mut scene,
    )
    .unwrap();
    assert_eq!(moved["value"], 0);
}

#[test]
fn rest_get_and_post_headers_and_body() {
    use std::{
        io::{BufRead, BufReader, Read, Write},
        net::TcpListener,
        time::Duration,
    };
    let server = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = server.local_addr().unwrap();
    let worker = std::thread::spawn(move || {
        for (method, body) in [("GET", ""), ("POST", "Rust payload")] {
            let (mut stream, _) = server.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(&stream);
            let mut first = String::new();
            reader.read_line(&mut first).unwrap();
            assert_eq!(first, format!("{method} /fixture HTTP/1.1\r\n"));
            let mut size = 0;
            let mut marker = false;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                assert!(!line.is_empty());
                if let Some(length) = line.strip_prefix("Content-Length:") {
                    size = length.trim().parse::<usize>().unwrap();
                }
                marker |= line == "X-Compat: runtime-test\r\n";
            }
            assert!(marker);
            let mut received = vec![0; size];
            reader.read_exact(&mut received).unwrap();
            assert_eq!(received, body.as_bytes());
            stream
                .write_all(b"HTTP/1.0 200 OK\r\nContent-Length: 7\r\n\r\nfixture")
                .unwrap();
        }
    });
    let mut scene = json!({});
    let url = format!("http://{address}/fixture");
    let headers = json!([["X-Compat", "runtime-test"]]);
    let get = tool(
        "clo_api_call",
        json!({"name":"rest_api.CallRESTGet","args":[url,headers,""]}),
        &mut scene,
    )
    .unwrap();
    let post = tool(
        "clo_api_call",
        json!({"name":"rest_api.CallRESTPost","args":[url,"Rust payload",headers]}),
        &mut scene,
    )
    .unwrap();
    assert_eq!(
        get["value"],
        "HTTP/1.0 200 OK\r\nContent-Length: 7\r\n\r\n\r\n\r\nfixture"
    );
    assert_eq!(get["value"], post["value"]);
    worker.join().unwrap();
}

#[test]
fn enum_constants_roots_and_typed_values() {
    let mut scene = json!({"fabrics":[{"color":"#ffffff"}]});
    let catalog = tool(
        "clo_api_catalog",
        json!({"query":"CLOAPI_ANCHOR_RIGHT","limit":10}),
        &mut scene,
    )
    .unwrap();
    assert_eq!(catalog["total"], 3);
    let constant = tool(
        "clo_api_call",
        json!({"name":"ApiTypes.CLOAPI_ANCHOR_RIGHT"}),
        &mut scene,
    )
    .unwrap();
    assert_eq!(
        constant["value"]["repr"],
        "<CloApiAnchorPoint.CLOAPI_ANCHOR_RIGHT: 3>"
    );
    let through_alias=tool("clo_api_call",json!({"name":"fabric_api.GetFabricPBRMaterialBaseColor","args":[0,{"$enum":"ApiTypes.MaterialTargetComponent.COMPONENT_FRONT"}]}),&mut scene).unwrap();
    assert_eq!(through_alias["value"], json!([1.0, 1.0, 1.0, 1.0]));
    let through_constructor = tool(
        "clo_api_call",
        json!({"name":"ApiTypes.CloApiAnchorPoint","args":[3]}),
        &mut scene,
    )
    .unwrap();
    assert_eq!(
        through_constructor["value"]["repr"],
        constant["value"]["repr"]
    );
    let script = tool(
        "clo_run_python",
        json!({"code":"[int(ApiTypes.CLOAPI_ANCHOR_RIGHT), ApiTypes.CloApiAnchorPoint(3).name]"}),
        &mut scene,
    )
    .unwrap();
    assert_eq!(script["value"], json!([3, "CLOAPI_ANCHOR_RIGHT"]));
    tool("clo_stop_bridge", json!({}), &mut scene).unwrap();
}

#[test]
fn unicode_name_queries_and_selection_share_workbench_state() {
    let mut scene = json!({"patterns":[{"id":3,"name":"前片"},{"id":4,"name":"后片"}],"seams":[{"name":"侧缝"}]});
    let get = |name: String, args: Value, state: &mut Value| {
        tool(
            "clo_api_call",
            json!({"name":format!("pattern_api.{name}"),"args":args}),
            state,
        )
        .unwrap()["value"]
            .clone()
    };
    assert_eq!(
        get("GetPatternIndexW".into(), json!(["后片"]), &mut scene),
        1
    );
    assert_eq!(
        get(
            "GetSeamlinePairGroupIndexFromNameW".into(),
            json!(["侧缝"]),
            &mut scene
        ),
        0
    );
    assert!(get(
        "SelectPatternViaName".into(),
        json!(["后片", true]),
        &mut scene
    )
    .is_null());
    assert!(get("SelectPatternViaIndex".into(), json!([0, true]), &mut scene).is_null());
    assert_eq!(
        get("GetSelectedPattern".into(), json!([]), &mut scene),
        json!(["前片", "后片"])
    );
    let state = tool("clo_ui_state", json!({}), &mut scene).unwrap();
    assert_eq!(
        state["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|node| node["AXRole"] == "AXRow" && node["AXValue"] == true)
            .count(),
        2
    );
    assert!(get(
        "SelectPatternViaIndex".into(),
        json!([0, false]),
        &mut scene
    )
    .is_null());
    scene["patterns"].as_array_mut().unwrap().remove(0);
    assert_eq!(
        get("GetSelectedPatternViaIndex".into(), json!([0]), &mut scene),
        true
    );
    assert_eq!(
        get("GetSelectedPattern".into(), json!([]), &mut scene),
        json!(["后片"])
    );
}

#[test]
fn avatar_rows_keep_python_attributes_and_raw_handles() {
    let mut scene = json!({"avatar":{"name":"Mia","body":{"height":1700.0,"waist":680.0}}});
    let row = tool("clo_run_python",json!({"code":"[(m.measurementName, m.value, m.unit) for m in utility_api.GetAvatarMeasurements(0)]"}),&mut scene).unwrap();
    assert_eq!(
        row["value"],
        json!([
            ["HEIGHT_Total", 1700.0, "mm"],
            ["CIRCUMFERENCE_Waist", 680.0, "mm"]
        ])
    );
    let raw = tool(
        "clo_api_call",
        json!({"name":"utility_api.GetAvatarMeasurements","args":[0]}),
        &mut scene,
    )
    .unwrap();
    assert_eq!(raw["value"][0]["type"], "CloApiAvatarMeasurement");
    let ids = raw["value"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["$handle"].clone())
        .collect::<Vec<_>>();
    let released = tool("clo_api_release", json!({"handles":ids}), &mut scene).unwrap();
    assert_eq!(released["released"], 2);
    let defaults = tool(
        "clo_run_python",
        json!({"code":"ApiTypes.CloApiAvatarMeasurement()"}),
        &mut scene,
    )
    .unwrap();
    assert_eq!(defaults["value"]["measurementName"], "");
    assert_eq!(defaults["value"]["value"], 0.0);
    tool("clo_stop_bridge", json!({}), &mut scene).unwrap();
}

#[test]
fn python_retains_completed_calls_before_script_failure() {
    let mut scene = json!({"patterns":[]});
    let code="created = pattern_api.CreatePatternWithPoints([(0,0),(40,0),(0,40)]); raise ValueError('expected-explicit-failure')";
    let error = tool("clo_run_python", json!({"code":code}), &mut scene).unwrap_err();
    assert!(error.contains("expected-explicit-failure"));
    assert_eq!(scene["patterns"].as_array().unwrap().len(), 1);
    let status = tool(
        "clo_run_python",
        json!({"code":"(created, pattern_api.GetPatternCount())"}),
        &mut scene,
    )
    .unwrap();
    assert_eq!(status["value"], json!([0, 1]));
    tool("clo_stop_bridge", json!({}), &mut scene).unwrap();
}
