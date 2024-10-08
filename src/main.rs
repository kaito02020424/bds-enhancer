pub mod action;
pub mod color;
pub mod consts;
pub mod log_level;
pub mod stream;

use json::{self, object};
use regex::Regex;
use std::env;
use std::io::Write;
use std::path::Path;
use std::process::{ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;

use action::Action;
use color::Color;
use consts::LOG_PREFIX;
use log_level::LogLevel;
use stream::LogDelimiterStream;

lazy_static::lazy_static! {
    static ref ACTION_MESSAGE_REGEX: Regex = Regex::new(r".*\[Scripting\] bds_enhancer:(?P<json>\{.*\})").unwrap();
    static ref LOG_REGEX: Regex = Regex::new(&format!(r"{} (?P<level>(INFO|WARN|ERROR))\] ", LOG_PREFIX)).unwrap();
}

fn handle_child_stdin(rx: Receiver<String>, mut child_stdin: ChildStdin) {
    loop {
        let input = rx.recv().unwrap();
        child_stdin
            .write_all(input.as_bytes())
            .expect("Failed to write to stdin");
    }
}

fn handle_stdin(child_stdin: Sender<String>) {
    let stdin = std::io::stdin();

    loop {
        let mut line = String::new();
        stdin.read_line(&mut line).unwrap();

        child_stdin.send(line).unwrap();
    }
}

fn get_log_level(log: &str) -> LogLevel {
    let level = LOG_REGEX
        .captures(log)
        .map(|caps| caps["level"].to_string())
        .unwrap_or("INFO".to_string());

    level.parse().unwrap()
}

fn parse_action(log: &str) -> Option<Action> {
    let caps = ACTION_MESSAGE_REGEX.captures(log)?;

    let json = caps.name("json").unwrap().as_str();
    serde_json::from_str(json).ok()?
}

fn handle_action(child_stdin: &Sender<String>, action: Action, command_status: &mut CommandStatus) {
    match action {
        Action::Transfer(arg) => execute_command(
            child_stdin,
            format!("transfer {} {} {}", arg.player, arg.host, arg.port),
        ),
        Action::Kick(arg) => {
            execute_command(child_stdin, format!("kick {} {}", arg.player, arg.reason))
        }
        Action::Reload => execute_command(child_stdin, "reload".to_string()),
        Action::Stop => execute_command(child_stdin, "stop".to_string()),
        Action::Execute(arg) => {
            if arg.result {
                command_status.waiting = true;
                command_status.command = arg.command.clone();
                command_status.scriptevent = "bds_enhancer:result".to_string();
            }
            execute_command(child_stdin, arg.command.to_string());
        }
    }
}

fn handle_child_stdout(
    child_stdin: Sender<String>,
    child_stdout: ChildStdout,
    mut command_status: &mut CommandStatus,
) {
    let logs = LogDelimiterStream::new(child_stdout);
    let mut stdout = std::io::stdout();

    for log in logs {
        if let Some(action) = parse_action(&log) {
            handle_action(&child_stdin, action, &mut command_status);
            continue;
        }

        let level = get_log_level(&log);

        let log = log.strip_prefix("NO LOG FILE! - ").unwrap_or(&log);
        if command_status.waiting {
            for i in 0..(log.chars().count() / 1500 + 1) {
                let result_tmp = log.chars().skip(i * 1500).take(1500).collect::<String>();
                let result: json::JsonValue = object! {
                    "command" => command_status.command.clone(),
                    "result_message" => result_tmp,
                    "count" => i,
                    "end" => i == log.chars().count() / 1500,
                };
                execute_command(
                    &child_stdin,
                    format!(
                        "scriptevent {} {} ",
                        command_status.scriptevent,
                        result.dump()
                    ),
                );
            }
            command_status.waiting = false;
        }
        let _ = stdout.write(format!("{}{}{}\n", level.to_color(), log, Color::Reset).as_bytes());
    }
}

fn execute_command(child_stdin: &Sender<String>, command: String) {
    child_stdin.send(format!("{}\n", command)).unwrap();
}

fn build_command(os: &str, cwd: &str) -> Command {
    if os != "linux" && os != "windows" {
        panic!("Unsupported platform: {}", os);
    }

    let mut command = Command::new(Path::new(cwd).join("bedrock_server"));

    command
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());

    if os == "linux" {
        command.env("LD_LIBRARY_PATH", ".");
    }

    command
}

fn main() {
    let os = env::consts::OS;
    let cwd = env::args().nth(1).unwrap_or(".".to_string());

    let mut child = build_command(os, &cwd)
        .spawn()
        .expect("Failed to spawn process");

    let child_stdin = child.stdin.take().expect("Failed to get stdin");
    let stdout = child.stdout.expect("Failed to get stdout");

    let (tx, rx) = channel::<String>();
    let tx2 = tx.clone();

    thread::spawn(move || handle_child_stdin(rx, child_stdin));
    thread::spawn(move || handle_stdin(tx));

    let mut command_status = CommandStatus {
        waiting: false,
        command: "".to_string(),
        scriptevent: "".to_string(),
    };
    handle_child_stdout(tx2, stdout, &mut command_status);
}

struct CommandStatus {
    waiting: bool,
    command: String,
    scriptevent: String,
}
