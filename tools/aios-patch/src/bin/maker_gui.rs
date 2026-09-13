#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(not(windows))]
fn main() {
    eprintln!("AIOS Patch Maker GUI currently supports Windows only.");
}

#[cfg(windows)]
mod windows_app {
    use std::fs::File;
    use std::io::Read;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use anyhow::{Context as _, Result, ensure};
    use gpui::{
        App, Application, Bounds, ClickEvent, Context, PathPromptOptions, SharedString, Window,
        WindowBounds, WindowOptions, div, prelude::*, px, rgb, size,
    };
    use serde::Deserialize;
    use sha2::{Digest, Sha256};

    const APP_NAME: &str = "AIOS Patch Maker";
    const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

    #[derive(Clone, Copy)]
    enum Slot {
        Source,
        Target,
    }

    impl Slot {
        fn title(self) -> &'static str {
            match self {
                Self::Source => "现场旧版",
                Self::Target => "目标新版",
            }
        }

        fn subtitle(self) -> &'static str {
            match self {
                Self::Source => "选择现场正在使用的 aios-database.exe",
                Self::Target => "选择准备升级到的 aios-database.exe",
            }
        }

        fn button_label(self) -> &'static str {
            match self {
                Self::Source => "选择旧版 EXE",
                Self::Target => "选择新版 EXE",
            }
        }

        fn prompt(self) -> &'static str {
            match self {
                Self::Source => "选择现场旧 aios-database.exe",
                Self::Target => "选择新版 aios-database.exe",
            }
        }

        fn step(self) -> &'static str {
            match self {
                Self::Source => "1",
                Self::Target => "2",
            }
        }

        fn button_id(self) -> &'static str {
            match self {
                Self::Source => "choose-source",
                Self::Target => "choose-target",
            }
        }
    }

    #[derive(Clone)]
    struct ExecutableDetails {
        path: PathBuf,
        version: String,
        build_id: String,
        sha256: String,
        bytes: u64,
    }

    #[derive(Clone, Default)]
    enum Selection {
        #[default]
        Empty,
        Inspecting(PathBuf),
        Ready(ExecutableDetails),
        Failed {
            path: PathBuf,
            message: String,
        },
    }

    impl Selection {
        fn ready(&self) -> Option<&ExecutableDetails> {
            match self {
                Self::Ready(details) => Some(details),
                _ => None,
            }
        }

        fn path_text(&self) -> String {
            match self {
                Self::Empty => "尚未选择文件".into(),
                Self::Inspecting(path) | Self::Failed { path, .. } => path.display().to_string(),
                Self::Ready(details) => details.path.display().to_string(),
            }
        }

        fn detail_text(&self) -> String {
            match self {
                Self::Empty => "选择后会自动读取版本、构建号与 SHA-256".into(),
                Self::Inspecting(_) => "正在读取版本身份并计算 SHA-256…".into(),
                Self::Ready(details) => format!(
                    "{}  ·  {}  ·  {}  ·  {}",
                    details.version,
                    details.build_id,
                    format_bytes(details.bytes),
                    &details.sha256[..12]
                ),
                Self::Failed { message, .. } => message.clone(),
            }
        }

        fn accent(&self) -> u32 {
            match self {
                Self::Empty => 0x64748b,
                Self::Inspecting(_) => 0x60a5fa,
                Self::Ready(_) => 0x34d399,
                Self::Failed { .. } => 0xfb7185,
            }
        }
    }

    enum Generation {
        Idle,
        Running,
        Succeeded(PathBuf),
        Failed(String),
    }

    struct PatchMaker {
        source: Selection,
        target: Selection,
        generation: Generation,
    }

    #[derive(Deserialize)]
    struct BuildInfo {
        product: String,
        version: String,
        build_id: String,
    }

    impl PatchMaker {
        fn new() -> Self {
            Self {
                source: Selection::Empty,
                target: Selection::Empty,
                generation: Generation::Idle,
            }
        }

        fn selection(&self, slot: Slot) -> &Selection {
            match slot {
                Slot::Source => &self.source,
                Slot::Target => &self.target,
            }
        }

        fn set_selection(&mut self, slot: Slot, selection: Selection) {
            match slot {
                Slot::Source => self.source = selection,
                Slot::Target => self.target = selection,
            }
            self.generation = Generation::Idle;
        }

        fn choose_file(
            &mut self,
            slot: Slot,
            _: &ClickEvent,
            _: &mut Window,
            cx: &mut Context<Self>,
        ) {
            if matches!(self.generation, Generation::Running) {
                return;
            }
            let receiver = cx.prompt_for_paths(PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some(SharedString::from(slot.prompt())),
            });
            let executor = cx.background_executor().clone();
            cx.spawn(async move |this, cx| {
                let selected = match receiver.await {
                    Ok(selected) => selected,
                    Err(_) => return,
                };
                let path = match selected {
                    Ok(Some(paths)) => paths.into_iter().next(),
                    Ok(None) => None,
                    Err(error) => {
                        this.update(cx, |view, cx| {
                            view.set_selection(
                                slot,
                                Selection::Failed {
                                    path: PathBuf::new(),
                                    message: format!("文件选择失败：{error}"),
                                },
                            );
                            cx.notify();
                        })
                        .ok();
                        return;
                    }
                };
                let Some(path) = path else {
                    return;
                };
                this.update(cx, |view, cx| {
                    view.set_selection(slot, Selection::Inspecting(path.clone()));
                    cx.notify();
                })
                .ok();

                let probe_path = path.clone();
                let result = executor
                    .spawn(async move { inspect_executable(&probe_path) })
                    .await;
                this.update(cx, |view, cx| {
                    let selection = match result {
                        Ok(details) => Selection::Ready(details),
                        Err(error) => Selection::Failed {
                            path,
                            message: format!("{error:#}"),
                        },
                    };
                    view.set_selection(slot, selection);
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }

        fn generate(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
            if matches!(self.generation, Generation::Running) {
                return;
            }
            let (Some(source), Some(target)) = (self.source.ready(), self.target.ready()) else {
                self.generation = Generation::Failed("请先选择并通过校验的旧版与新版 EXE".into());
                cx.notify();
                return;
            };
            if source.sha256 == target.sha256 {
                self.generation = Generation::Failed("两个 EXE 完全相同，不需要生成 patch".into());
                cx.notify();
                return;
            }
            let source = source.path.clone();
            let target = target.path.clone();
            let output_dir = source
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."));
            self.generation = Generation::Running;
            cx.notify();

            let executor = cx.background_executor().clone();
            cx.spawn(async move |this, cx| {
                let result = executor
                    .spawn(
                        async move { aios_patch::make_patch_for_ui(&source, &target, &output_dir) },
                    )
                    .await;
                this.update(cx, |view, cx| {
                    view.generation = match result {
                        Ok(patch) => Generation::Succeeded(patch),
                        Err(error) => Generation::Failed(format!("{error:#}")),
                    };
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }

        fn reveal_output(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
            if let Generation::Succeeded(path) = &self.generation {
                cx.reveal_path(path);
            }
        }

        fn file_card(&self, slot: Slot, cx: &mut Context<Self>) -> impl IntoElement {
            let selection = self.selection(slot);
            let path = selection.path_text();
            let detail = selection.detail_text();
            let accent = selection.accent();
            let disabled = matches!(self.generation, Generation::Running);

            div()
                .w_full()
                .rounded_xl()
                .border_1()
                .border_color(rgb(0x24324b))
                .bg(rgb(0x111c2e))
                .p_5()
                .flex()
                .flex_col()
                .gap_4()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_3()
                                .child(
                                    div()
                                        .size_8()
                                        .rounded_full()
                                        .bg(rgb(accent))
                                        .text_color(rgb(0x07111f))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(slot.step()),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .child(
                                            div()
                                                .text_lg()
                                                .text_color(rgb(0xf8fafc))
                                                .child(slot.title()),
                                        )
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(rgb(0x94a3b8))
                                                .child(slot.subtitle()),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .id(slot.button_id())
                                .px_4()
                                .py_2()
                                .rounded_md()
                                .border_1()
                                .border_color(rgb(0x3b82f6))
                                .text_color(rgb(0xdbeafe))
                                .bg(rgb(0x172554))
                                .when(!disabled, |button| {
                                    button
                                        .cursor_pointer()
                                        .hover(|style| style.bg(rgb(0x1e3a8a)))
                                        .on_click(cx.listener(move |view, event, window, cx| {
                                            view.choose_file(slot, event, window, cx)
                                        }))
                                })
                                .when(disabled, |button| button.opacity(0.45))
                                .child(slot.button_label()),
                        ),
                )
                .child(
                    div()
                        .rounded_lg()
                        .bg(rgb(0x0b1424))
                        .border_1()
                        .border_color(rgb(0x1e293b))
                        .px_4()
                        .py_3()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgb(0xe2e8f0))
                                .overflow_hidden()
                                .child(path),
                        )
                        .child(div().text_sm().text_color(rgb(accent)).child(detail)),
                )
        }

        fn generation_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
            let ready = self.source.ready().is_some() && self.target.ready().is_some();
            let running = matches!(self.generation, Generation::Running);
            let (headline, detail, color) = match &self.generation {
                Generation::Idle => (
                    "准备生成".to_string(),
                    "补丁会写到旧版 EXE 同目录，并自动完成四道自检。".to_string(),
                    0x94a3b8,
                ),
                Generation::Running => (
                    "正在生成并验证 Patch…".to_string(),
                    "正在计算 bsdiff，并执行 dry-run、真实替换、备份哈希和二次幂等验证。"
                        .to_string(),
                    0x60a5fa,
                ),
                Generation::Succeeded(path) => (
                    "Patch 已生成并通过全部验证".to_string(),
                    path.display().to_string(),
                    0x34d399,
                ),
                Generation::Failed(error) => ("生成失败".to_string(), error.clone(), 0xfb7185),
            };

            div()
                .w_full()
                .rounded_xl()
                .border_1()
                .border_color(rgb(0x24324b))
                .bg(rgb(0x0f1929))
                .p_5()
                .flex()
                .items_center()
                .justify_between()
                .gap_5()
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(div().text_lg().text_color(rgb(color)).child(headline))
                        .child(div().text_sm().text_color(rgb(0x94a3b8)).child(detail)),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .when(matches!(self.generation, Generation::Succeeded(_)), |row| {
                            row.child(
                                div()
                                    .id("reveal-output")
                                    .cursor_pointer()
                                    .px_4()
                                    .py_3()
                                    .rounded_lg()
                                    .border_1()
                                    .border_color(rgb(0x334155))
                                    .text_color(rgb(0xe2e8f0))
                                    .hover(|style| style.bg(rgb(0x1e293b)))
                                    .on_click(cx.listener(Self::reveal_output))
                                    .child("打开所在目录"),
                            )
                        })
                        .child(
                            div()
                                .id("generate-patch")
                                .px_5()
                                .py_3()
                                .rounded_lg()
                                .text_color(rgb(0xffffff))
                                .bg(if ready && !running {
                                    rgb(0x2563eb)
                                } else {
                                    rgb(0x334155)
                                })
                                .when(ready && !running, |button| {
                                    button
                                        .cursor_pointer()
                                        .hover(|style| style.bg(rgb(0x1d4ed8)))
                                        .on_click(cx.listener(Self::generate))
                                })
                                .when(!ready || running, |button| button.opacity(0.6))
                                .child(if running {
                                    "生成中…"
                                } else {
                                    "生成 Patch"
                                }),
                        ),
                )
        }
    }

    impl Render for PatchMaker {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let engine_note = format!("内置 Patch Engine {APP_VERSION} · 单文件运行");

            div()
                .size_full()
                .bg(rgb(0x09111f))
                .text_color(rgb(0xe2e8f0))
                .flex()
                .flex_col()
                .child(
                    div()
                        .w_full()
                        .border_b_1()
                        .border_color(rgb(0x1e293b))
                        .bg(rgb(0x0b1424))
                        .px_8()
                        .py_5()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_4()
                                .child(
                                    div()
                                        .size_10()
                                        .rounded_xl()
                                        .bg(rgb(0x2563eb))
                                        .text_color(rgb(0xffffff))
                                        .text_xl()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child("P"),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .child(
                                            div()
                                                .text_2xl()
                                                .text_color(rgb(0xf8fafc))
                                                .child(APP_NAME),
                                        )
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(rgb(0x94a3b8))
                                                .child("精确哈希门控 · 自包含二进制差分升级"),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .rounded_full()
                                .border_1()
                                .border_color(rgb(0x334155))
                                .px_3()
                                .py_1()
                                .text_sm()
                                .text_color(rgb(0x93c5fd))
                                .child(format!("v{APP_VERSION} · GPUI")),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .w_full()
                        .px_8()
                        .py_6()
                        .flex()
                        .flex_col()
                        .gap_5()
                        .child(self.file_card(Slot::Source, cx))
                        .child(self.file_card(Slot::Target, cx))
                        .child(self.generation_panel(cx))
                        .child(
                            div()
                                .w_full()
                                .flex()
                                .justify_between()
                                .items_center()
                                .text_sm()
                                .text_color(rgb(0x64748b))
                                .child(engine_note)
                                .child("不会修改所选的旧版与新版 EXE"),
                        ),
                )
        }
    }

    fn inspect_executable(path: &Path) -> Result<ExecutableDetails> {
        ensure!(
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("exe")),
            "请选择 .exe 文件"
        );
        let output = Command::new(path)
            .args(["version", "--json"])
            .current_dir(path.parent().unwrap_or_else(|| Path::new(".")))
            .output()
            .with_context(|| format!("运行 {} version --json", path.display()))?;
        ensure!(
            output.status.success(),
            "版本读取失败：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        let info: BuildInfo = serde_json::from_slice(&output.stdout).context("版本 JSON 无效")?;
        ensure!(
            info.product == "aios-database",
            "所选 EXE 不是 aios-database"
        );
        let metadata = path.metadata()?;
        let sha256 = sha256_file(path)?;
        Ok(ExecutableDetails {
            path: path.to_path_buf(),
            version: info.version,
            build_id: info.build_id,
            sha256,
            bytes: metadata.len(),
        })
    }

    fn sha256_file(path: &Path) -> Result<String> {
        let mut file = File::open(path)?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; 1024 * 1024];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
        }
        Ok(hex::encode(hasher.finalize()))
    }

    fn format_bytes(bytes: u64) -> String {
        if bytes >= 1024 * 1024 {
            format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
        } else if bytes >= 1024 {
            format!("{:.1} KB", bytes as f64 / 1024.0)
        } else {
            format!("{bytes} B")
        }
    }

    pub fn run() {
        Application::new().run(|cx: &mut App| {
            cx.on_window_closed(|cx| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            let bounds = Bounds::centered(None, size(px(960.0), px(720.0)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    focus: true,
                    ..Default::default()
                },
                |_, cx| cx.new(|_| PatchMaker::new()),
            )
            .expect("open Patch Maker window");
            cx.activate(true);
        });
    }
}

#[cfg(windows)]
fn show_message_box(title: &str, message: &str, is_error: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MessageBoxW,
    };

    let title = title
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let message = message
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let style = MB_OK
        | if is_error {
            MB_ICONERROR
        } else {
            MB_ICONINFORMATION
        };
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            message.as_ptr(),
            title.as_ptr(),
            style,
        );
    }
}

#[cfg(windows)]
fn main() {
    let has_arguments = std::env::args_os().nth(1).is_some();
    if has_arguments || aios_patch::current_executable_has_bundle() {
        let result = aios_patch::run_cli();
        if !has_arguments {
            match &result {
                Ok(()) => show_message_box(
                    "AIOS Database Patch",
                    "升级已完成。旧版 EXE 已保留为带哈希后缀的备份文件。",
                    false,
                ),
                Err(error) => show_message_box(
                    "AIOS Database Patch - 升级失败",
                    &format!("{error:#}"),
                    true,
                ),
            }
        }
        if let Err(error) = result {
            if has_arguments {
                eprintln!("{error:#}");
            }
            std::process::exit(1);
        }
        return;
    }

    windows_app::run();
}
