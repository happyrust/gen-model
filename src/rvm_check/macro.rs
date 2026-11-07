//! 只读 RVM 导出宏。句子与 `scripts/e3d/LinkageE3dCommon.ps1` 的
//! `New-RvmExportMacro` 同一套窄口径：保温和障碍关、管子开、孔打开。
//! 不写 `QUIT` / `SAVEWORK`，会话归 `E3dDriver` 的包装宏。

use std::path::Path;

use anyhow::{Result, ensure};

use crate::e3d_query::{e3d_path, validate_refno};

pub fn login_probe_body() -> &'static str {
    "$P RVM-CHECK-LOGIN-OK\n"
}

/// 用 refno（`=a/b`）导航并导出。
pub fn export_body(refno: &str, rvm_file: &Path) -> Result<String> {
    let refno = validate_refno(refno)?;
    let path = export_path(rvm_file)?;
    let element = format!("={refno}");
    Ok(render(&element, &path, &element))
}

/// refno 导出没有落出文件时的退路：按 `Q NAME` 读到的名字再导一次。
pub fn export_body_by_name(name: &str, rvm_file: &Path) -> Result<String> {
    let name = safe_element_name(name)?;
    let path = export_path(rvm_file)?;
    Ok(render(&name, &path, &name))
}

/// 一次会话里逐个导出多个根（批量对拍：一个库的全部 SITE / APPLDW 共用一次登录）。
///
/// 表现级与导出开关只设一次；每个根各自 `export file … export finish`，并在后面打一个带
/// refno 的哨兵 [`export_done_marker`]，调用方按哨兵 + 文件大小逐根判有没有导出来。
/// `onerror continue` 让一个根导不出（不在 MDB、没名字）时后面的根照常导，单根失败由
/// 调用方再按 [`export_body`] / [`export_body_by_name`] 单独补一次。
pub fn export_batch_body(roots: &[(String, std::path::PathBuf)]) -> Result<String> {
    ensure!(!roots.is_empty(), "批量导出至少要一个根");
    let mut body = String::from(
        "onerror continue\n\
         repre insu off\n\
         repre obst off\n\
         repre tube on\n\
         representation level 6\n\
         representation level hvac 6\n\
         export implied tube into separate containers\n\
         export system /expdri.so\n",
    );
    for (refno, rvm_file) in roots {
        let refno = validate_refno(refno.trim().trim_start_matches('='))?;
        let path = export_path(rvm_file)?;
        body.push_str(&format!(
            "\n\
             ={refno}\n\
             Q CE\n\
             Q NAME\n\
             Q TYPE\n\
             export file \"{path}\"\n\
             export holes on\n\
             export autocolour displayexport on\n\
             export repr on\n\
             export ={refno}\n\
             export finish\n\
             $P {}\n",
            export_done_marker(&refno)
        ));
    }
    body.push_str("$P RVM-CHECK-EXPORT-DONE\n");
    Ok(body)
}

/// 批量导出里某个根做完的哨兵行。
pub fn export_done_marker(refno: &str) -> String {
    format!("RVM-CHECK-EXPORT-DONE ={refno}")
}

/// 批量导出的日志里有没有这个根的哨兵。
pub fn export_done_for(log: &str, refno: &str) -> bool {
    let marker = export_done_marker(refno);
    log.lines().any(|line| line.trim() == marker)
}

fn render(navigate: &str, rvm: &str, export_element: &str) -> String {
    format!(
        "{navigate}\n\
         Q CE\n\
         Q NAME\n\
         Q TYPE\n\
         \n\
         repre insu off\n\
         repre obst off\n\
         repre tube on\n\
         representation level 6\n\
         representation level hvac 6\n\
         export implied tube into separate containers\n\
         export system /expdri.so\n\
         export file \"{rvm}\"\n\
         export holes on\n\
         export autocolour displayexport on\n\
         export repr on\n\
         export {export_element}\n\
         export finish\n\
         $P RVM-CHECK-EXPORT-DONE\n"
    )
}

fn export_path(rvm_file: &Path) -> Result<String> {
    let path = e3d_path(rvm_file);
    ensure!(!path.is_empty(), "RVM 输出路径为空");
    ensure!(
        !path.chars().any(|ch| matches!(ch, '"' | '\n' | '\r')),
        "RVM 输出路径含有引号或换行"
    );
    Ok(path)
}

/// 名字要能原样放进宏：以 `/` 开头，不含引号、变量符和管道符。
pub fn safe_element_name(name: &str) -> Result<String> {
    let name = name.trim();
    ensure!(name.starts_with('/'), "元素名不是 / 开头: {name}");
    ensure!(name.len() > 1, "元素名为空");
    ensure!(
        name.chars().all(|ch| {
            ch.is_ascii_graphic()
                && !matches!(ch, '"' | '$' | '!' | ';' | '|' | '&' | '<' | '>' | '\\')
        }),
        "元素名含有不能写进宏的字符: {name}"
    );
    Ok(name.to_string())
}

pub fn parse_element_name(log: &str) -> Option<String> {
    log.lines().map(str::trim).find_map(|line| {
        let name = line.strip_prefix("Name ")?.trim();
        name.starts_with('/').then(|| name.to_string())
    })
}

pub fn export_finished(log: &str) -> bool {
    log.contains("RVM-CHECK-EXPORT-DONE")
}

pub fn login_ok(log: &str) -> bool {
    log.contains("RVM-CHECK-LOGIN-OK")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn export_macro_is_read_only_and_addresses_the_refno() {
        let body = export_body("24381/145018", Path::new(r"D:\out\e3d.rvm")).unwrap();
        assert!(body.contains("=24381/145018\n"));
        assert!(body.contains("export =24381/145018\n"));
        assert!(body.contains("export file \"D:/out/e3d.rvm\"\n"));
        assert!(body.contains("repre insu off\n"));
        assert!(body.contains("export holes on\n"));
        assert!(body.contains("$P RVM-CHECK-EXPORT-DONE\n"));
        for forbidden in ["QUIT", "SAVEWORK", "SAVE WORK"] {
            assert!(
                !body
                    .lines()
                    .any(|line| line.trim().eq_ignore_ascii_case(forbidden)),
                "{forbidden} leaked into {body}"
            );
        }
    }

    /// 没有这两行时 E3D 会把 EQUI 的隐式 BOX 展成 `/GENERATED/BOX n of EQUI ...`
    /// 成员，和我们生成器的缺省表现级（e3d-model `RepreDiscipline::default_level`）
    /// 对不上，拍出成百条假 `only_generated`。顺序要求在 export 之前，否则对
    /// 后续元素的表现级不生效。
    #[test]
    fn export_macro_sets_representation_level_before_export() {
        let body = export_body("24381/145018", Path::new(r"D:\out\e3d.rvm")).unwrap();
        assert!(body.contains("representation level 6\n"));
        assert!(body.contains("representation level hvac 6\n"));
        let level_pos = body
            .find("representation level 6\n")
            .expect("representation level 6 missing");
        let level_hvac_pos = body
            .find("representation level hvac 6\n")
            .expect("representation level hvac 6 missing");
        let export_pos = body.find("export ").expect("export statement missing");
        assert!(
            level_pos < export_pos && level_hvac_pos < export_pos,
            "representation level lines must come before export statements"
        );
    }

    #[test]
    fn refno_with_injection_is_rejected() {
        assert!(export_body("24381/1; QUIT", Path::new("e3d.rvm")).is_err());
    }

    /// 批量宏：开关只设一次、每个根各自 `export file … finish` 并带自己的哨兵，一个根坏了
    /// 不许打断后面的（`onerror continue`），依旧不许带会话命令。
    #[test]
    fn batch_macro_exports_each_root_into_its_own_file_and_marks_it() {
        let roots = vec![
            (
                "24383/66456".to_string(),
                PathBuf::from(r"D:\out\a\e3d.rvm"),
            ),
            (
                "=24383/73927".to_string(),
                PathBuf::from(r"D:\out\b\e3d.rvm"),
            ),
        ];
        let body = export_batch_body(&roots).unwrap();
        assert!(body.starts_with("onerror continue\n"));
        assert_eq!(body.matches("export system /expdri.so").count(), 1);
        assert_eq!(body.matches("representation level 6\n").count(), 1);
        assert!(body.contains("export file \"D:/out/a/e3d.rvm\"\nexport holes on\n"));
        assert!(body.contains(
            "export =24383/66456\nexport finish\n$P RVM-CHECK-EXPORT-DONE =24383/66456\n"
        ));
        assert!(body.contains(
            "export =24383/73927\nexport finish\n$P RVM-CHECK-EXPORT-DONE =24383/73927\n"
        ));
        assert!(body.ends_with("$P RVM-CHECK-EXPORT-DONE\n"));
        for forbidden in ["QUIT", "SAVEWORK", "SAVE WORK"] {
            assert!(
                !body
                    .lines()
                    .any(|line| line.trim().eq_ignore_ascii_case(forbidden))
            );
        }
        let log = "export finish\nRVM-CHECK-EXPORT-DONE =24383/66456\n$P RVM-CHECK-EXPORT-DONE\n";
        assert!(export_done_for(log, "24383/66456"));
        assert!(!export_done_for(log, "24383/73927"));
        assert!(export_batch_body(&[]).is_err());
        assert!(export_batch_body(&[("1/2;QUIT".to_string(), PathBuf::from("x.rvm"))]).is_err());
    }

    #[test]
    fn name_fallback_rejects_unsafe_characters() {
        assert!(export_body_by_name("/C-OR-1", Path::new("e3d.rvm")).is_ok());
        assert!(export_body_by_name("C-OR-1", Path::new("e3d.rvm")).is_err());
        assert!(export_body_by_name("/A\"B", Path::new("e3d.rvm")).is_err());
        assert!(export_body_by_name("/A$B", Path::new("e3d.rvm")).is_err());
    }

    #[test]
    fn name_is_read_from_the_query_line() {
        let log = "Q NAME\nName /C-OR-1R345-C\nQ TYPE\nType BRAN\n";
        assert_eq!(parse_element_name(log).as_deref(), Some("/C-OR-1R345-C"));
        assert!(export_finished("export finish\n$P RVM-CHECK-EXPORT-DONE\n"));
        assert!(!export_finished("export finish\n"));
    }
}
