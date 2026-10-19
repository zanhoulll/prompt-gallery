//! 应用内测试钩子（ADR-0008）：后台注入合成输入、GPU 回读自截图、文件 IPC 指令通道。
//!
//! - 只有 debug 构建才编译真实现；release 构建拿到零尺寸 stub（from_env 恒为 None）。
//! - 启用方式：把环境变量 [`imp::ENV_VAR`] 指向一个目录；不设置时零行为变化。
//! - 协议与限制见 docs/adr/0008-in-app-test-hook-for-machine-checks.md；
//!   驱动脚本为 tools/hookdrive.py。

#[cfg(debug_assertions)]
mod imp {
    use std::collections::VecDeque;
    use std::ffi::OsString;
    use std::io::Write as _;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use egui::{ColorImage, Event, Key, Modifiers, PointerButton, Pos2, UserData, ViewportCommand};

    /// 启用测试钩子的环境变量：指向钩子目录（内含 requests.jsonl / responses.jsonl / shots/）。
    pub const ENV_VAR: &str = "PROMPT_GALLERY_TEST_HOOK";
    /// 钩子启用时的重绘间隔：egui 是反应式的，空闲时没有帧，命令就推不动。
    const REPAINT_INTERVAL: Duration = Duration::from_millis(16);
    const NEWLINE: u8 = 10;
    const CARRIAGE_RETURN: u8 = 13;
    const MAX_SHOT_NAME: usize = 64;

    /// 驱动可以发来的一条命令。
    #[derive(Debug, Clone, PartialEq)]
    pub enum Command {
        Move { x: f32, y: f32 },
        Down { x: f32, y: f32 },
        Up { x: f32, y: f32 },
        Click { x: f32, y: f32 },
        Text { text: String },
        Key { key: Key, ctrl: bool, shift: bool, alt: bool },
        Shot { name: String },
        Wait { ms: u64 },
        /// 让应用走一遍「选了一张图」的入库路径（原生文件对话框无法被合成输入驱动）。
        Image { path: String },
        Quit,
    }

    impl Command {
        /// 应答里回写的命令名。
        pub fn name(&self) -> &'static str {
            match self {
                Command::Move { .. } => "move",
                Command::Down { .. } => "down",
                Command::Up { .. } => "up",
                Command::Click { .. } => "click",
                Command::Text { .. } => "text",
                Command::Key { .. } => "key",
                Command::Shot { .. } => "shot",
                Command::Wait { .. } => "wait",
                Command::Image { .. } => "image",
                Command::Quit => "quit",
            }
        }
    }

    /// 已解析、等待执行的一条命令；frames 是它跨帧的事件计划。
    struct Queued {
        seq: u64,
        cmd: Command,
        frames: VecDeque<Vec<Event>>,
        started: Instant,
    }

    /// 等待 GPU 回读的一次截图。
    struct AwaitingShot {
        seq: u64,
        name: String,
        started: Instant,
    }

    /// 应用内测试钩子。未启用时调用方拿到 None，不产生任何行为。
    pub struct TestHook {
        requests: PathBuf,
        shots: PathBuf,
        responses: std::fs::File,
        read_offset: u64,
        pending_line: String,
        next_seq: u64,
        queue: VecDeque<Queued>,
        current: Option<Queued>,
        pending_responses: VecDeque<String>,
        awaiting_shot: Option<AwaitingShot>,
        wait_until: Option<(u64, Instant, Instant)>,
        /// 已交给应用的「选图」请求（应用处理完自己应答）。
        image_request: Option<(u64, String, Instant)>,
        image_started: Option<(u64, Instant)>,
        /// 保活脉冲线程的停止开关；见 start_pulse。
        pulse: Option<Arc<AtomicBool>>,
    }

    impl TestHook {
        /// 由环境变量决定是否启用；目录建不出来时打印一行并返回 None（不 panic）。
        pub fn from_env() -> Option<Self> {
            let dir = dir_from_env_value(std::env::var_os(ENV_VAR))?;
            let shots = dir.join("shots");
            if let Err(err) = std::fs::create_dir_all(&shots) {
                eprintln!("测试钩子：无法创建目录 {}：{err}", shots.display());
                return None;
            }
            let responses = match std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("responses.jsonl"))
            {
                Ok(file) => file,
                Err(err) => {
                    eprintln!("测试钩子：无法打开应答文件：{err}");
                    return None;
                }
            };
            Some(Self {
                requests: dir.join("requests.jsonl"),
                shots,
                responses,
                read_offset: 0,
                pending_line: String::new(),
                next_seq: 0,
                queue: VecDeque::new(),
                current: None,
                pending_responses: VecDeque::new(),
                awaiting_shot: None,
                wait_until: None,
                image_request: None,
                image_started: None,
                pulse: None,
            })
        }

        /// 每帧一次：先冲刷上一帧攒下的应答，再处理截图与命令，最后保证还有下一帧。
        pub fn tick(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
            if self.pulse.is_none() {
                self.start_pulse(ctx);
            }
            self.flush_responses();
            self.collect_screenshots(raw_input);
            self.finish_wait();
            if self.awaiting_shot.is_none() && self.wait_until.is_none() {
                if self.current.is_none() {
                    self.poll_requests(ctx.pixels_per_point());
                }
                self.advance(ctx, raw_input);
            }
            ctx.request_repaint_after(REPAINT_INTERVAL);
        }

        /// 启动保活脉冲线程。
        ///
        /// egui 是反应式的：一旦应用进入空闲等待就没有帧，raw_input_hook 也不再被调用。
        /// 而在 hook 里 request_repaint_after 的请求会被 eframe 按 cumulative_pass_nr
        /// 判为过期丢弃（eframe-0.36.2/src/native/run.rs:320-344），所以必须有
        /// pass 之外的唤醒源。脉冲线程每 16ms 请求一次重绘，保证钩子启用期间一直有帧。
        fn start_pulse(&mut self, ctx: &egui::Context) {
            let stop = Arc::new(AtomicBool::new(false));
            let flag = Arc::clone(&stop);
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                while !flag.load(Ordering::Relaxed) {
                    ctx.request_repaint();
                    std::thread::sleep(REPAINT_INTERVAL);
                }
            });
            self.pulse = Some(stop);
        }

        /// 读取 requests.jsonl 自上次偏移之后的新字节，按行切分后入队。
        fn poll_requests(&mut self, pixels_per_point: f32) {
            let data = match std::fs::read(&self.requests) {
                Ok(data) => data,
                Err(_) => return,
            };
            if (data.len() as u64) < self.read_offset {
                self.read_offset = 0;
                self.pending_line.clear();
            }
            if (data.len() as u64) == self.read_offset {
                return;
            }
            let chunk = &data[self.read_offset as usize..];
            self.read_offset = data.len() as u64;
            self.pending_line.push_str(&String::from_utf8_lossy(chunk));
            for line in drain_lines(&mut self.pending_line) {
                self.next_seq += 1;
                let seq = self.next_seq;
                let started = Instant::now();
                match parse_command(&line) {
                    Ok(cmd) => {
                        let frames = command_to_frames(&cmd, pixels_per_point);
                        self.queue.push_back(Queued { seq, cmd, frames, started });
                    }
                    Err(err) => self.push_response(seq, "?", Err(err), started),
                }
            }
        }

        /// 执行队首命令的一步：注入一帧事件，或启动一次 wait / shot / quit。
        fn advance(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
            if self.current.is_none() {
                self.current = self.queue.pop_front();
            }
            let Some(mut current) = self.current.take() else {
                return;
            };
            match current.cmd.clone() {
                Command::Shot { name } => {
                    let name = sanitize_shot_name(&name);
                    ctx.send_viewport_cmd(ViewportCommand::Screenshot(UserData::new(current.seq)));
                    self.awaiting_shot = Some(AwaitingShot { seq: current.seq, name, started: current.started });
                }
                Command::Wait { ms } => {
                    self.wait_until = Some((
                        current.seq,
                        Instant::now() + Duration::from_millis(ms),
                        current.started,
                    ));
                }
                Command::Image { path } => {
                    // 真正的处理在应用侧（它才知道当前示例与锁状态），这里只把请求递过去。
                    self.image_request = Some((current.seq, path, current.started));
                }
                Command::Quit => {
                    self.push_response(current.seq, "quit", Ok(String::new()), current.started);
                    self.flush_responses();
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
                _ => {
                    if let Some(frame) = current.frames.pop_front() {
                        raw_input.events.extend(frame);
                    }
                    if current.frames.is_empty() {
                        let name = current.cmd.name();
                        self.push_response(current.seq, name, Ok(String::new()), current.started);
                    } else {
                        self.current = Some(current);
                    }
                }
            }
        }

        /// 取出待处理的「选图」请求（应用每帧开头调用）。
        pub fn take_image_request(&mut self) -> Option<(u64, String)> {
            let (seq, path, started) = self.image_request.take()?;
            self.image_started = Some((seq, started));
            Some((seq, path))
        }

        /// 应用走完选图路径后应答；`result` 只用于轨迹记录，不影响场景执行。
        pub fn finish_image_request(&mut self, result: Result<String, String>) {
            if let Some((seq, started)) = self.image_started.take() {
                self.push_response(seq, "image", result, started);
            }
        }

        /// 处理本帧 raw_input.events 里的截图回包：写 PNG 并应答。
        fn collect_screenshots(&mut self, raw_input: &egui::RawInput) {
            let mut taken: Option<Arc<ColorImage>> = None;
            for event in &raw_input.events {
                if let Event::Screenshot { user_data, image, .. } = event {
                    let seq = match user_data
                        .data
                        .as_ref()
                        .and_then(|data| data.downcast_ref::<u64>().copied())
                    {
                        Some(seq) => seq,
                        None => continue,
                    };
                    if matches!(&self.awaiting_shot, Some(shot) if shot.seq == seq) {
                        taken = Some(Arc::clone(image));
                        break;
                    }
                }
            }
            let Some(image) = taken else {
                return;
            };
            if let Some(shot) = self.awaiting_shot.take() {
                let result = save_png(&self.shots, &shot.name, &image)
                    .map(|()| format!("shots/{}.png", shot.name));
                self.push_response(shot.seq, "shot", result, shot.started);
            }
        }

        /// wait 到点：本次先把应答排队，下一帧才真正写出（保证那帧的 App::ui 已经跑完）。
        fn finish_wait(&mut self) {
            if let Some((seq, deadline, started)) = self.wait_until {
                if Instant::now() >= deadline {
                    self.wait_until = None;
                    self.push_response(seq, "wait", Ok(String::new()), started);
                }
            }
        }

        fn push_response(&mut self, seq: u64, cmd: &str, result: Result<String, String>, started: Instant) {
            let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
            self.pending_responses
                .push_back(response_line(seq, cmd, &result, elapsed_ms));
        }

        /// 冲刷待写应答；写失败不 panic（驱动会因超时发现）。
        fn flush_responses(&mut self) {
            while let Some(line) = self.pending_responses.pop_front() {
                let _ = self.responses.write_all(line.as_bytes());
                let _ = self.responses.write_all(&[NEWLINE]);
            }
            let _ = self.responses.flush();
        }
    }

    impl Drop for TestHook {
        fn drop(&mut self) {
            if let Some(stop) = self.pulse.take() {
                stop.store(true, Ordering::Relaxed);
            }
        }
    }

    /// 解析一行请求；任何毛病都返回可读错误，绝不 panic。
    pub fn parse_command(line: &str) -> Result<Command, String> {
        let value: serde_json::Value =
            serde_json::from_str(line).map_err(|err| format!("非法 JSON：{err}"))?;
        let obj = value
            .as_object()
            .ok_or_else(|| "命令行必须是 JSON 对象".to_string())?;
        let cmd = obj
            .get("cmd")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "缺少字符串字段 cmd".to_string())?;
        match cmd {
            "move" => Ok(Command::Move { x: number_field(obj, "x")?, y: number_field(obj, "y")? }),
            "down" => Ok(Command::Down { x: number_field(obj, "x")?, y: number_field(obj, "y")? }),
            "up" => Ok(Command::Up { x: number_field(obj, "x")?, y: number_field(obj, "y")? }),
            "click" => Ok(Command::Click { x: number_field(obj, "x")?, y: number_field(obj, "y")? }),
            "text" => Ok(Command::Text { text: string_field(obj, "text")? }),
            "key" => {
                let name = string_field(obj, "key")?;
                let key = parse_key(&name).ok_or_else(|| format!("未知键名：{name}"))?;
                Ok(Command::Key {
                    key,
                    ctrl: flag_field(obj, "ctrl"),
                    shift: flag_field(obj, "shift"),
                    alt: flag_field(obj, "alt"),
                })
            }
            "shot" => Ok(Command::Shot { name: string_field(obj, "name")? }),
            "image" => Ok(Command::Image { path: string_field(obj, "path")? }),
            "wait" => {
                let ms = obj
                    .get("ms")
                    .and_then(|value| value.as_u64())
                    .ok_or_else(|| "字段 ms 必须是非负整数".to_string())?;
                Ok(Command::Wait { ms })
            }
            "quit" => Ok(Command::Quit),
            other => Err(format!("未知命令：{other}")),
        }
    }

    fn number_field(obj: &serde_json::Map<String, serde_json::Value>, key: &str) -> Result<f32, String> {
        match obj.get(key).and_then(|value| value.as_f64()) {
            Some(value) => Ok(value as f32),
            None => Err(format!("字段 {key} 必须是数字")),
        }
    }

    fn string_field(obj: &serde_json::Map<String, serde_json::Value>, key: &str) -> Result<String, String> {
        match obj.get(key).and_then(|value| value.as_str()) {
            Some(value) => Ok(value.to_string()),
            None => Err(format!("字段 {key} 必须是字符串")),
        }
    }

    fn flag_field(obj: &serde_json::Map<String, serde_json::Value>, key: &str) -> bool {
        obj.get(key).and_then(|value| value.as_bool()).unwrap_or(false)
    }

    /// 键名先按 egui 原名解析，失败再试大写形式（兼容驱动写 r 而不是 R）。
    fn parse_key(name: &str) -> Option<Key> {
        Key::from_name(name).or_else(|| Key::from_name(&name.to_ascii_uppercase()))
    }

    /// 把缓冲里所有完整行取出；末尾不完整的部分留到下次。
    pub fn drain_lines(pending: &mut String) -> Vec<String> {
        let mut lines = Vec::new();
        let newline = char::from(NEWLINE);
        let carriage_return = char::from(CARRIAGE_RETURN);
        while let Some(index) = pending.find(newline) {
            let raw: String = pending.drain(..=index).collect();
            let line = raw.trim_end_matches(|ch: char| ch == newline || ch == carriage_return).trim();
            if !line.is_empty() {
                lines.push(line.to_string());
            }
        }
        lines
    }

    /// 一条命令展开成每帧一组事件：click 两帧、key 三帧、其余一帧；shot/wait/quit 不发事件。
    pub fn command_to_frames(cmd: &Command, pixels_per_point: f32) -> VecDeque<Vec<Event>> {
        let scale = if pixels_per_point > 0.0 { pixels_per_point } else { 1.0 };
        let mut frames = VecDeque::new();
        match cmd {
            Command::Move { x, y } => frames.push_back(vec![Event::PointerMoved(to_pos(*x, *y, scale))]),
            Command::Down { x, y } => {
                frames.push_back(vec![pointer_button(to_pos(*x, *y, scale), true, Modifiers::NONE)])
            }
            Command::Up { x, y } => {
                frames.push_back(vec![pointer_button(to_pos(*x, *y, scale), false, Modifiers::NONE)])
            }
            Command::Click { x, y } => {
                let pos = to_pos(*x, *y, scale);
                frames.push_back(vec![pointer_button(pos, true, Modifiers::NONE)]);
                frames.push_back(vec![pointer_button(pos, false, Modifiers::NONE)]);
            }
            Command::Text { text } => frames.push_back(vec![Event::Text(text.clone())]),
            Command::Key { key, ctrl, shift, alt } => {
                let modifiers = Modifiers {
                    alt: *alt,
                    ctrl: *ctrl,
                    shift: *shift,
                    command: *ctrl,
                    ..Modifiers::NONE
                };
                frames.push_back(vec![Event::ModifiersChanged(modifiers), key_event(*key, true, modifiers)]);
                frames.push_back(vec![key_event(*key, false, modifiers)]);
                frames.push_back(vec![Event::ModifiersChanged(Modifiers::NONE)]);
            }
            Command::Shot { .. } | Command::Wait { .. } | Command::Image { .. } | Command::Quit => {}
        }
        frames
    }

    fn to_pos(x: f32, y: f32, pixels_per_point: f32) -> Pos2 {
        Pos2::new(x / pixels_per_point, y / pixels_per_point)
    }

    fn pointer_button(pos: Pos2, pressed: bool, modifiers: Modifiers) -> Event {
        Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers }
    }

    fn key_event(key: Key, pressed: bool, modifiers: Modifiers) -> Event {
        Event::Key { key, physical_key: None, pressed, repeat: false, modifiers }
    }

    /// 文件名净化：只留 ASCII 字母数字与 - _ .，其余变 -；空或全点号回退到 shot。
    pub fn sanitize_shot_name(name: &str) -> String {
        let mut out = String::new();
        for ch in name.chars().take(MAX_SHOT_NAME) {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
                out.push(ch);
            } else {
                out.push('-');
            }
        }
        if let Some(stripped) = out.strip_suffix(".png") {
            out = stripped.to_string();
        }
        let trimmed = out.trim_end_matches(|ch: char| ch == '.' || ch == '-').to_string();
        if trimmed.is_empty() || trimmed.chars().all(|ch| ch == '.') {
            "shot".to_string()
        } else {
            trimmed
        }
    }

    /// shot 的落盘路径：shots/<净化名>.png，永远留在 shots 目录内。
    pub fn shot_path(shots_dir: &Path, name: &str) -> PathBuf {
        shots_dir.join(format!("{}.png", sanitize_shot_name(name)))
    }

    /// 环境变量取值：未设置或空串都视为未启用。
    pub fn dir_from_env_value(value: Option<OsString>) -> Option<PathBuf> {
        let value = value?;
        if value.is_empty() {
            return None;
        }
        Some(PathBuf::from(value))
    }

    /// 一行应答 JSON：{"seq":N,"cmd":"move","ok":true,"ms":0.4,"detail":""}。
    pub fn response_line(seq: u64, cmd: &str, result: &Result<String, String>, elapsed_ms: f64) -> String {
        let mut obj = serde_json::Map::new();
        obj.insert("seq".to_string(), serde_json::Value::from(seq));
        obj.insert("cmd".to_string(), serde_json::Value::from(cmd));
        match result {
            Ok(detail) => {
                obj.insert("ok".to_string(), serde_json::Value::Bool(true));
                obj.insert("detail".to_string(), serde_json::Value::from(detail.as_str()));
            }
            Err(err) => {
                obj.insert("ok".to_string(), serde_json::Value::Bool(false));
                obj.insert("error".to_string(), serde_json::Value::from(err.as_str()));
            }
        }
        let ms = (elapsed_ms * 10.0).round() / 10.0;
        obj.insert("ms".to_string(), serde_json::Value::from(ms));
        serde_json::Value::Object(obj).to_string()
    }

    /// ColorImage 落成 PNG：GPU 回读是预乘的，写盘前反预乘。
    pub fn save_png(shots_dir: &Path, name: &str, image: &ColorImage) -> Result<(), String> {
        let [width, height] = image.size;
        let mut rgba = Vec::with_capacity(width * height * 4);
        for pixel in &image.pixels {
            rgba.extend_from_slice(&pixel.to_srgba_unmultiplied());
        }
        let path = shot_path(shots_dir, name);
        let buffer = image::RgbaImage::from_raw(width as u32, height as u32, rgba)
            .ok_or_else(|| "截图缓冲区尺寸不匹配".to_string())?;
        buffer.save(&path).map_err(|err| format!("写入 PNG 失败：{err}"))
    }
}

#[cfg(debug_assertions)]
pub use imp::TestHook;

#[cfg(not(debug_assertions))]
pub struct TestHook;

#[cfg(not(debug_assertions))]
impl TestHook {
    pub fn from_env() -> Option<Self> {
        None
    }

    pub fn tick(&mut self, _ctx: &egui::Context, _raw_input: &mut egui::RawInput) {}

    pub fn take_image_request(&mut self) -> Option<(u64, String)> {
        None
    }

    pub fn finish_image_request(&mut self, _result: Result<String, String>) {}
}

#[cfg(all(test, debug_assertions))]
mod tests {
    use super::imp::*;
    use egui::{ColorImage, Event, Key, Modifiers, PointerButton};
    use std::path::{Path, PathBuf};

    fn parse(line: &str) -> Command {
        parse_command(line).expect("应能解析")
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "prompt-gallery-testhook-{}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    #[test]
    fn parse_all_commands() {
        assert_eq!(parse(r#"{"cmd":"move","x":1,"y":2}"#), Command::Move { x: 1.0, y: 2.0 });
        assert_eq!(parse(r#"{"cmd":"down","x":1,"y":2}"#), Command::Down { x: 1.0, y: 2.0 });
        assert_eq!(parse(r#"{"cmd":"up","x":1,"y":2}"#), Command::Up { x: 1.0, y: 2.0 });
        assert_eq!(parse(r#"{"cmd":"click","x":1,"y":2}"#), Command::Click { x: 1.0, y: 2.0 });
        assert_eq!(parse(r#"{"cmd":"text","text":"你好"}"#), Command::Text { text: "你好".to_string() });
        assert_eq!(
            parse(r#"{"cmd":"key","key":"R","ctrl":true}"#),
            Command::Key { key: Key::R, ctrl: true, shift: false, alt: false }
        );
        assert_eq!(parse(r#"{"cmd":"shot","name":"K1-hover"}"#), Command::Shot { name: "K1-hover".to_string() });
        assert_eq!(parse(r#"{"cmd":"wait","ms":700}"#), Command::Wait { ms: 700 });
        assert_eq!(
            parse(r#"{"cmd":"image","path":"E:/tmp/a.png"}"#),
            Command::Image { path: "E:/tmp/a.png".to_string() }
        );
        assert_eq!(parse(r#"{"cmd":"quit"}"#), Command::Quit);
    }

    #[test]
    fn parse_rejects_bad_input() {
        assert!(parse_command("not json").is_err());
        assert!(parse_command("[1,2]").is_err());
        assert!(parse_command(r#"{}"#).is_err());
        assert!(parse_command(r#"{"cmd":"nope"}"#).is_err());
        assert!(parse_command(r#"{"cmd":"move","x":1}"#).is_err());
        assert!(parse_command(r#"{"cmd":"move","x":"a","y":1}"#).is_err());
        assert!(parse_command(r#"{"cmd":"text"}"#).is_err());
        assert!(parse_command(r#"{"cmd":"key","key":"NoSuchKey"}"#).is_err());
        assert!(parse_command(r#"{"cmd":"wait","ms":-1}"#).is_err());
    }

    #[test]
    fn key_names_accept_letter_case_variants() {
        for name in ["R", "r"] {
            let line = format!(r#"{{"cmd":"key","key":"{name}"}}"#);
            assert_eq!(
                parse(&line),
                Command::Key { key: Key::R, ctrl: false, shift: false, alt: false }
            );
        }
        assert_eq!(
            parse(r#"{"cmd":"key","key":"Escape"}"#),
            Command::Key { key: Key::Escape, ctrl: false, shift: false, alt: false }
        );
    }

    #[test]
    fn drain_lines_keeps_the_partial_tail() {
        let mut pending = String::new();
        pending.push_str("alpha
beta
par");
        let lines = drain_lines(&mut pending);
        assert_eq!(lines, vec!["alpha".to_string(), "beta".to_string()]);
        assert_eq!(pending, "par");
        pending.push_str("tial

");
        assert_eq!(drain_lines(&mut pending), vec!["partial".to_string()]);
        assert!(pending.is_empty());
    }

    #[test]
    fn frames_convert_pixels_to_points_and_button_state() {
        let frames = command_to_frames(&parse(r#"{"cmd":"move","x":269,"y":117}"#), 1.25);
        assert_eq!(frames.len(), 1);
        match &frames[0][0] {
            Event::PointerMoved(pos) => {
                assert!((pos.x - 215.2).abs() < 0.01, "x 应为 215.2pt，实际 {}", pos.x);
                assert!((pos.y - 93.6).abs() < 0.01, "y 应为 93.6pt，实际 {}", pos.y);
            }
            other => panic!("应为 PointerMoved，实际 {other:?}"),
        }
        let down = command_to_frames(&parse(r#"{"cmd":"down","x":10,"y":20}"#), 1.0);
        match &down[0][0] {
            Event::PointerButton { pressed, button, .. } => {
                assert!(*pressed);
                assert_eq!(*button, PointerButton::Primary);
            }
            other => panic!("应为 PointerButton，实际 {other:?}"),
        }
        let up = command_to_frames(&parse(r#"{"cmd":"up","x":10,"y":20}"#), 1.0);
        match &up[0][0] {
            Event::PointerButton { pressed, .. } => assert!(!*pressed),
            other => panic!("应为 PointerButton，实际 {other:?}"),
        }
        let text = command_to_frames(&parse(r#"{"cmd":"text","text":"abc"}"#), 1.0);
        match &text[0][0] {
            Event::Text(value) => assert_eq!(value, "abc"),
            other => panic!("应为 Text，实际 {other:?}"),
        }
    }

    #[test]
    fn click_and_key_plans_span_frames() {
        assert_eq!(command_to_frames(&parse(r#"{"cmd":"click","x":1,"y":1}"#), 1.0).len(), 2);
        let frames = command_to_frames(&parse(r#"{"cmd":"key","key":"R","ctrl":true}"#), 1.0);
        assert_eq!(frames.len(), 3);
        let mut saw_modifiers = false;
        let mut saw_key = false;
        for event in &frames[0] {
            match event {
                Event::ModifiersChanged(modifiers) => {
                    assert!(modifiers.ctrl);
                    assert!(modifiers.command);
                    saw_modifiers = true;
                }
                Event::Key { key, pressed, modifiers, .. } => {
                    assert_eq!(*key, Key::R);
                    assert!(*pressed);
                    assert!(modifiers.ctrl);
                    saw_key = true;
                }
                _ => {}
            }
        }
        assert!(saw_modifiers && saw_key);
        assert!(matches!(&frames[1][0], Event::Key { pressed, .. } if !*pressed));
        match &frames[2][0] {
            Event::ModifiersChanged(modifiers) => assert_eq!(*modifiers, Modifiers::NONE),
            other => panic!("收尾应清空修饰键，实际 {other:?}"),
        }
    }

    #[test]
    fn shot_names_never_escape_the_shots_dir() {
        assert_eq!(sanitize_shot_name("K1-hover"), "K1-hover");
        assert_eq!(sanitize_shot_name("K1-hover.png"), "K1-hover");
        for raw in ["../evil", "a/b", "..", ".", "", "///"] {
            let name = sanitize_shot_name(raw);
            assert!(!name.contains('/'), "不应含路径分隔符：{name}");
            assert!(name != ".." && name != ".");
            let path = shot_path(Path::new("shots"), raw);
            assert_eq!(path.parent(), Some(Path::new("shots")));
            assert_eq!(path.extension().and_then(|ext| ext.to_str()), Some("png"));
        }
        let backslash = char::from(92);
        let with_backslash = format!("a{backslash}b");
        assert!(!sanitize_shot_name(&with_backslash).contains(backslash));
        assert_eq!(sanitize_shot_name("..."), "shot");
        assert_eq!(sanitize_shot_name(""), "shot");
    }

    #[test]
    fn env_dir_value_requires_non_empty() {
        assert!(dir_from_env_value(None).is_none());
        assert!(dir_from_env_value(Some(std::ffi::OsString::new())).is_none());
        assert_eq!(
            dir_from_env_value(Some(std::ffi::OsString::from("target/hook"))),
            Some(PathBuf::from("target/hook"))
        );
    }

    #[test]
    fn response_line_is_single_line_json() {
        let line = response_line(7, "move", &Ok(String::new()), 1.25);
        assert!(!line.contains(char::from(10)));
        let value: serde_json::Value = serde_json::from_str(&line).expect("合法 JSON");
        assert_eq!(value["seq"], 7);
        assert_eq!(value["cmd"], "move");
        assert_eq!(value["ok"], true);
        assert_eq!(value["ms"], 1.3);
        let failed = response_line(8, "?", &Err("坏了".to_string()), 0.0);
        let value: serde_json::Value = serde_json::from_str(&failed).expect("合法 JSON");
        assert_eq!(value["ok"], false);
        assert_eq!(value["error"], "坏了");
    }

    #[test]
    fn save_png_writes_a_real_png() {
        let dir = temp_dir("png");
        let image = ColorImage::filled([2, 2], egui::Color32::from_rgb(10, 20, 30));
        save_png(&dir, "unit-1", &image).expect("写 PNG");
        let path = dir.join("unit-1.png");
        let bytes = std::fs::read(&path).expect("读回 PNG");
        assert_eq!(&bytes[..8], &[137, 80, 78, 71, 13, 10, 26, 10]);
        assert_eq!(image::image_dimensions(&path).expect("PNG 尺寸"), (2, 2));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
