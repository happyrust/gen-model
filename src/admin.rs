use std::fs;
use std::path::PathBuf;

pub async fn filter_sys_file(project_dir: &str) -> anyhow::Result<()> {
    let mut target_dir = fs::read_dir(&project_dir)
        .unwrap()
        .into_iter()
        .map(|entry| {
            let entry = entry.unwrap();
            entry.path()
        })
        .find(|x| x.is_dir() && x.file_name().unwrap().to_str().unwrap().ends_with("000"))
        .unwrap();

    let children_files = fs::read_dir(target_dir)?
        .into_iter()
        .map(|entry| {
            let entry = entry.unwrap();
            entry.path()
        })
        .collect::<Vec<_>>();

    let mut sys_path = vec![];
    for children_file in children_files {
        let filename = children_file.file_name();
        if filename.is_none() {
            continue;
        }
        let filename = filename.unwrap().to_str();
        if filename.is_none() {
            continue;
        }
        let filename = filename.unwrap();
        if !filename.ends_with("sys") {
            continue;
        }
        sys_path.push(children_file);
    }

    Ok(())
}
