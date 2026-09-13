from pathlib import Path

p = Path('src/data_interface/increment_manager.rs')
s = p.read_text(encoding='utf-8')
a = s.index('            let project = self.db_option.project_name.clone();', s.index('    async fn sweep_watch_dirs('))
b = s.index('            log::info!(\n                "[{origin}] read-through data face:', a)
s = s[:a] + '''            // run_read_through_core owns generation polling and targeted model refresh.
            // A filesystem notification must not start a second MDB-wide refresh.
''' + s[b:]
a = s.index('        // Direct tree/attribute readers use the same in-memory MDB snapshot')
b = s.index('        let watch_dirs = self.watch_dirs();', a)
s = s[:a] + '''        // Direct readers check their source identities on demand. Ingest discovery
        // does not build or refresh an element skeleton.
''' + s[b:]
s = s.replace('region.contains(".with_dbnum("),', '''region.contains(".with_dbnum(")
                    || (region.contains("SYS 文件解析失败:") && region.contains(".with_project(")),''', 1)
s = s.replace('"阶段 blocker 必须带上它自己的 dbnum: {region}"', '"阶段 blocker 必须带 dbnum；SYS 头损坏、库号未知时必须带 project: {region}"', 1)
p.write_text(s, encoding='utf-8')

p = Path('src/web_service/handlers.rs')
s = p.read_text(encoding='utf-8')
at = s.index('    if sul_db.get("connected").and_then(Value::as_bool) == Some(false) {', s.index('pub async fn health('))
s = s[:at] + '''    let mut core = crate::data_interface::mdb_route_directory::active(
        &state.identity.project, &state.identity.mdb,
    ).map(|directory| directory.health()).unwrap_or_else(|| json!({
        "status": "initializing", "route_count": 0, "snapshot_epoch": 0,
        "session_vector": [], "stale_dbnums": [], "failed_dbnums": [],
        "last_targeted_refresh": null,
    }));
    let (mut opened, attribute_elements) = state.element_attributes.cache_stats().await;
    let (tree_opened, tree_elements) = state.direct_tree.cache_stats().await;
    opened.extend(tree_opened);
    opened.sort_unstable();
    opened.dedup();
    core["open_db_count"] = json!(opened.len());
    core["open_dbnums"] = json!(opened);
    core["cached_elements"] = json!(attribute_elements + tree_elements);
''' + s[at:]
a = s.index('        let snapshot = crate::data_interface::mdb_memory_store::get(', at)
b = s.index('        return Json(json!({', a)
s = s[:a] + s[b:]
a = s.index('            "status": if snapshot.is_some()', a)
b = s.index('            "mirror": {', a)
s = s[:a] + '''            "status": if core["status"] == "ready" { "ok" } else { "degraded" },
            "core": core,
''' + s[b:]
a = s.index('    let source_snapshot =', a)
b = s.index('    Json(json!({', a)
s = s[:a] + s[b:]
a = s.index('        "core": {', a)
b = s.index('        "mirror": {', a)
s = s[:a] + '''        "core": core,
''' + s[b:]
a = s.index('        assert_eq!(\n            index.matches("window.location.origin").count(),')
b = s.index('\n    }', a)
s = s[:a] + '''        // Assert the URL fallbacks themselves, not an incidental total count:
        // the embedded page supports both legacy and current config shapes.
        assert!(index.contains("configuredModelUrl !== \\\"http://127.0.0.1:8022\\\""));
        assert!(index.contains("current.data_api_url?.trim() || window.location.origin"));
        assert!(index.contains("current.data_api_url = configuredDataUrl || window.location.origin"));
        assert!(index.contains(": window.location.origin"));
''' + s[b:]
p.write_text(s, encoding='utf-8')
