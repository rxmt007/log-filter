mod commands;
mod dto;
mod problems;
mod state;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(state::AppState::new())
        .invoke_handler(tauri::generate_handler![
            commands::open_file,
            commands::list_devices,
            commands::start_logcat,
            commands::pause_logcat,
            commands::resume_logcat,
            commands::stop_logcat,
            commands::clear_logcat,
            commands::get_status,
            problems::get_problems_status,
            problems::get_problem_groups,
            problems::get_problem_occurrences,
            problems::get_problem_detail,
            problems::release_problem_snapshot,
            commands::get_rows,
            commands::get_rows_checked,
            commands::map_source_line,
            commands::set_filter,
            commands::get_filtered_count,
            commands::search,
            commands::search_next,
            commands::toggle_bookmark,
            commands::list_bookmarks,
            commands::next_bookmark,
            commands::line_to_result_index,
            commands::get_minimap,
            commands::export_logs,
            commands::export_problem_logs,
            commands::cancel_export,
            commands::split_log_file,
            commands::get_config,
            commands::set_config
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};
    use std::io::Write;
    use std::time::{Duration, Instant};
    use tauri::ipc::{CallbackFn, InvokeBody};
    use tauri::test::{get_ipc_response, mock_builder, mock_context, noop_assets, INVOKE_KEY};
    use tauri::webview::InvokeRequest;
    use tauri::{Manager, WebviewWindow};

    fn invoke_json(
        webview: &WebviewWindow<tauri::test::MockRuntime>,
        command: &str,
        body: Value,
    ) -> Result<Value, Value> {
        get_ipc_response(
            webview,
            InvokeRequest {
                cmd: command.into(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: if cfg!(any(windows, target_os = "android")) {
                    "http://tauri.localhost"
                } else {
                    "tauri://localhost"
                }
                .parse()
                .unwrap(),
                body: InvokeBody::Json(body),
                headers: Default::default(),
                invoke_key: INVOKE_KEY.to_string(),
            },
        )
        .map(|response| response.deserialize::<Value>().unwrap())
    }

    fn wait_for_finished_problems(webview: &WebviewWindow<tauri::test::MockRuntime>) -> Value {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let status = invoke_json(webview, "get_problems_status", json!({})).unwrap();
            if status["finished"] == json!(true) {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "Problems analysis did not finish before the test deadline: {status}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn page_ids(page: &Value, field: &str) -> Vec<u64> {
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item[field].as_u64().unwrap())
            .collect()
    }

    fn lmk_line(minute: u32, second: u32, process: &str, pid: u32) -> String {
        format!(
            "07-26 18:{minute:02}:{second:02}.000  900  900 I lmkd: Kill '{process}' ({pid}), uid 10601, oom_score_adj 900 to free 1000kB rss, 0kB swap; reason: low watermark\n"
        )
    }

    #[test]
    fn group_and_occurrence_ipc_pages_remain_frozen_after_analysis_revision_growth() {
        let directory = tempfile::tempdir().unwrap();
        let log_path = directory.path().join("growing.log");
        let mut initial = String::new();
        for (second, process, pid) in [
            (1, "com.example.one", 601),
            (2, "com.example.two", 602),
            (3, "com.example.three", 603),
            (4, "com.example.four", 604),
            (5, "com.example.repeat", 701),
            (6, "com.example.repeat", 702),
            (7, "com.example.repeat", 703),
            (8, "com.example.repeat", 704),
        ] {
            initial.push_str(&lmk_line(0, second, process, pid));
        }
        initial.push_str(
            "07-26 18:10:00.000  900  900 I Example: advance the live analysis watermark\n",
        );
        for _ in 0..4_100 {
            initial.push_str("07-26 18:10:00.001  900  900 I Example: ordinary live padding\n");
        }
        std::fs::write(&log_path, initial).unwrap();

        let app = mock_builder()
            .manage(crate::state::AppState::new())
            .invoke_handler(tauri::generate_handler![
                crate::problems::get_problems_status,
                crate::problems::get_problem_groups,
                crate::problems::get_problem_occurrences
            ])
            .build(mock_context(noop_assets()))
            .unwrap();
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();

        let mut session = logcore::session::Session::open_growing(&log_path).unwrap();
        session
            .add_problem_source_span(
                logcore::problems::SourceSpan::new(0, u32::MAX, logcore::problems::LogBuffer::Main)
                    .unwrap(),
            )
            .unwrap();
        session.index_all();
        while !session.scan_problems_step(4_096).caught_up {}
        assert_eq!(session.problem_stats().stored_occurrence_count, 8);
        app.state::<crate::state::AppState>()
            .replace_session(session);
        let before = invoke_json(&webview, "get_problems_status", json!({})).unwrap();
        let analysis_token = before["analysisToken"].clone();
        let before_revision = before["stats"]["revision"].as_u64().unwrap();

        let baseline_groups = invoke_json(
            &webview,
            "get_problem_groups",
            json!({
                "request": {
                    "expectedAnalysisToken": analysis_token,
                    "sort": "last-seen-desc",
                    "limit": 100
                }
            }),
        )
        .unwrap();
        let expected_group_ids = page_ids(&baseline_groups, "id");
        assert_eq!(expected_group_ids.len(), 5);
        let repeat_group_id = baseline_groups["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["observedOccurrenceCount"] == json!(4))
            .unwrap()["id"]
            .as_u64()
            .unwrap();

        let first_groups = invoke_json(
            &webview,
            "get_problem_groups",
            json!({
                "request": {
                    "expectedAnalysisToken": analysis_token,
                    "sort": "last-seen-desc",
                    "limit": 2
                }
            }),
        )
        .unwrap();
        let frozen_group_revision = first_groups["revision"].as_u64().unwrap();
        let frozen_group_handle = first_groups["snapshotHandle"].clone();
        let mut frozen_group_ids = page_ids(&first_groups, "id");
        let mut group_cursor = first_groups["nextCursor"].as_str().map(str::to_owned);

        let baseline_occurrences = invoke_json(
            &webview,
            "get_problem_occurrences",
            json!({
                "request": {
                    "expectedAnalysisToken": analysis_token,
                    "groupId": repeat_group_id,
                    "limit": 100
                }
            }),
        )
        .unwrap();
        let expected_event_ids = page_ids(&baseline_occurrences, "eventId");
        assert_eq!(expected_event_ids.len(), 4);
        let first_occurrences = invoke_json(
            &webview,
            "get_problem_occurrences",
            json!({
                "request": {
                    "expectedAnalysisToken": analysis_token,
                    "groupId": repeat_group_id,
                    "limit": 2
                }
            }),
        )
        .unwrap();
        let frozen_occurrence_revision = first_occurrences["revision"].as_u64().unwrap();
        let frozen_occurrence_handle = first_occurrences["snapshotHandle"].clone();
        let mut frozen_event_ids = page_ids(&first_occurrences, "eventId");
        let mut occurrence_cursor = first_occurrences["nextCursor"].as_str().map(str::to_owned);

        let mut writer = std::fs::OpenOptions::new()
            .append(true)
            .open(&log_path)
            .unwrap();
        writer
            .write_all(lmk_line(11, 1, "com.example.five", 605).as_bytes())
            .unwrap();
        writer
            .write_all(lmk_line(11, 2, "com.example.repeat", 705).as_bytes())
            .unwrap();
        writer
            .write_all(
                b"07-26 18:20:00.000  900  900 I Example: advance the live analysis watermark\n",
            )
            .unwrap();
        for _ in 0..4_100 {
            writer
                .write_all(b"07-26 18:20:00.001  900  900 I Example: ordinary live padding\n")
                .unwrap();
        }
        writer.flush().unwrap();
        {
            let state = app.state::<crate::state::AppState>();
            let mut guard = state.lock_session();
            let session = guard.as_mut().unwrap();
            session.remap_and_index_step(usize::MAX).unwrap();
            while !session.scan_problems_step(4_096).caught_up {}
        }
        let after = invoke_json(&webview, "get_problems_status", json!({})).unwrap();
        assert!(after["stats"]["revision"].as_u64().unwrap() > before_revision);

        while let Some(cursor) = group_cursor {
            let page = invoke_json(
                &webview,
                "get_problem_groups",
                json!({
                    "request": {
                        "expectedAnalysisToken": analysis_token,
                        "sort": "last-seen-desc",
                        "cursor": cursor,
                        "limit": 2
                    }
                }),
            )
            .unwrap();
            assert_eq!(page["snapshotHandle"], frozen_group_handle);
            assert_eq!(page["revision"], json!(frozen_group_revision));
            frozen_group_ids.extend(page_ids(&page, "id"));
            group_cursor = page["nextCursor"].as_str().map(str::to_owned);
        }
        assert_eq!(frozen_group_ids, expected_group_ids);

        while let Some(cursor) = occurrence_cursor {
            let page = invoke_json(
                &webview,
                "get_problem_occurrences",
                json!({
                    "request": {
                        "expectedAnalysisToken": analysis_token,
                        "groupId": repeat_group_id,
                        "cursor": cursor,
                        "limit": 2
                    }
                }),
            )
            .unwrap();
            assert_eq!(page["snapshotHandle"], frozen_occurrence_handle);
            assert_eq!(page["revision"], json!(frozen_occurrence_revision));
            frozen_event_ids.extend(page_ids(&page, "eventId"));
            occurrence_cursor = page["nextCursor"].as_str().map(str::to_owned);
        }
        assert_eq!(frozen_event_ids, expected_event_ids);

        let refreshed_groups = invoke_json(
            &webview,
            "get_problem_groups",
            json!({
                "request": {
                    "expectedAnalysisToken": analysis_token,
                    "sort": "last-seen-desc",
                    "limit": 100
                }
            }),
        )
        .unwrap();
        assert_eq!(
            refreshed_groups["total"],
            json!(expected_group_ids.len() + 1)
        );
        let refreshed_occurrences = invoke_json(
            &webview,
            "get_problem_occurrences",
            json!({
                "request": {
                    "expectedAnalysisToken": analysis_token,
                    "groupId": repeat_group_id,
                    "limit": 100
                }
            }),
        )
        .unwrap();
        assert_eq!(
            refreshed_occurrences["total"],
            json!(expected_event_ids.len() + 1)
        );
    }

    #[test]
    fn detail_and_locate_reject_an_event_from_a_replaced_analysis() {
        let directory = tempfile::tempdir().unwrap();
        let first_log = directory.path().join("first.log");
        std::fs::write(&first_log, lmk_line(0, 1, "com.example.stale", 601)).unwrap();
        let second_log = directory.path().join("second.log");
        std::fs::write(
            &second_log,
            "07-26 18:00:02.000  900  900 I Example: replacement session\n",
        )
        .unwrap();

        let app = mock_builder()
            .manage(crate::state::AppState::new())
            .invoke_handler(tauri::generate_handler![
                crate::problems::get_problems_status,
                crate::problems::get_problem_groups,
                crate::problems::get_problem_occurrences,
                crate::problems::get_problem_detail,
                crate::commands::map_source_line
            ])
            .build(mock_context(noop_assets()))
            .unwrap();
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();

        let mut first_session = logcore::session::Session::open(&first_log).unwrap();
        first_session.index_all();
        while !first_session.scan_problems_step(4_096).caught_up {}
        assert!(first_session.finish_problem_input().finished);
        app.state::<crate::state::AppState>()
            .replace_session(first_session);
        let status = wait_for_finished_problems(&webview);
        let old_token = status["analysisToken"].clone();
        let groups = invoke_json(
            &webview,
            "get_problem_groups",
            json!({
                "request": {
                    "expectedAnalysisToken": old_token,
                    "sort": "last-seen-desc",
                    "limit": 100
                }
            }),
        )
        .unwrap();
        let group_id = groups["items"][0]["id"].as_u64().unwrap();
        let occurrences = invoke_json(
            &webview,
            "get_problem_occurrences",
            json!({
                "request": {
                    "expectedAnalysisToken": old_token,
                    "groupId": group_id,
                    "limit": 100
                }
            }),
        )
        .unwrap();
        let event_id = occurrences["items"][0]["eventId"].as_u64().unwrap();
        let anchor_line = occurrences["items"][0]["anchorLine"].as_u64().unwrap();

        let mut replacement = logcore::session::Session::open(&second_log).unwrap();
        replacement.index_all();
        app.state::<crate::state::AppState>()
            .replace_session(replacement);

        assert_eq!(
            invoke_json(
                &webview,
                "get_problem_detail",
                json!({
                    "request": {
                        "eventId": event_id,
                        "expectedAnalysisToken": old_token
                    }
                })
            ),
            Err(json!("stale-analysis-token"))
        );
        assert_eq!(
            invoke_json(
                &webview,
                "map_source_line",
                json!({
                    "request": {
                        "lineNo": anchor_line,
                        "bias": "exact",
                        "expectedAnalysisToken": old_token,
                        "expectedFilterResultRevision": 0,
                        "requestNonce": 1
                    }
                })
            ),
            Err(json!("stale-analysis-token"))
        );
    }

    #[test]
    fn releasing_a_problem_snapshot_is_idempotent_without_weakening_capability_errors() {
        let directory = tempfile::tempdir().unwrap();
        let first_log = directory.path().join("first.log");
        std::fs::write(
            &first_log,
            "07-26 18:00:01.000  900  900 I lmkd: Kill 'com.example.memory' (601), uid 10601, oom_score_adj 900 to free 1000kB rss, 0kB swap; reason: low watermark\n",
        )
        .unwrap();
        let second_log = directory.path().join("second.log");
        std::fs::write(
            &second_log,
            "07-26 18:00:02.000  900  900 I Example: replacement session\n",
        )
        .unwrap();

        let app = mock_builder()
            .manage(crate::state::AppState::new())
            .invoke_handler(tauri::generate_handler![
                crate::problems::get_problems_status,
                crate::problems::get_problem_groups,
                crate::problems::release_problem_snapshot
            ])
            .build(mock_context(noop_assets()))
            .unwrap();
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();

        let mut first_session = logcore::session::Session::open(&first_log).unwrap();
        first_session.index_all();
        while !first_session.scan_problems_step(4_096).caught_up {}
        assert!(first_session.finish_problem_input().finished);
        app.state::<crate::state::AppState>()
            .replace_session(first_session);
        let status = wait_for_finished_problems(&webview);
        let analysis_token = status["analysisToken"].clone();
        let groups = invoke_json(
            &webview,
            "get_problem_groups",
            json!({
                "request": {
                    "expectedAnalysisToken": analysis_token,
                    "sort": "last-seen-desc",
                    "limit": 100
                }
            }),
        )
        .unwrap();
        assert_eq!(groups["items"].as_array().unwrap().len(), 1);
        let snapshot_handle = groups["snapshotHandle"].clone();
        let release_request = json!({
            "request": {
                "snapshotHandle": snapshot_handle,
                "expectedAnalysisToken": analysis_token
            }
        });

        assert_eq!(
            invoke_json(
                &webview,
                "release_problem_snapshot",
                release_request.clone()
            ),
            Ok(json!(true))
        );
        assert_eq!(
            invoke_json(
                &webview,
                "release_problem_snapshot",
                release_request.clone()
            ),
            Ok(json!(false))
        );
        assert_eq!(
            invoke_json(
                &webview,
                "release_problem_snapshot",
                json!({
                    "request": {
                        "snapshotHandle": "not-a-problem-snapshot",
                        "expectedAnalysisToken": analysis_token
                    }
                })
            ),
            Err(json!("problem-cursor-invalid"))
        );

        let mut second_session = logcore::session::Session::open(&second_log).unwrap();
        second_session.index_all();
        app.state::<crate::state::AppState>()
            .replace_session(second_session);
        assert_eq!(
            invoke_json(&webview, "release_problem_snapshot", release_request),
            Err(json!("stale-analysis-token"))
        );
    }
}
