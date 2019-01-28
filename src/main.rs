use std::fs::create_dir;
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::time::SystemTime;

use std::fs::OpenOptions;
use std::path::PathBuf;
use std::process::Command;

use chrono::{Date, Local};

static DELIM: &'static str = ";";

#[derive(Debug)]
enum DaemonError {
    XpropWinIdParse,
    XpropClassParse,
}

//activity frame
#[derive(Debug)]
pub struct Frame {
    name: String,
    start: u64,
    end: u64,
}

#[derive(Debug)]
struct CurrentState {
    last_frame: Option<Frame>,
    last_line_length: usize,
    last_date: Date<Local>,
    file: File,
}

impl CurrentState {
    pub fn new(path: &PathBuf) -> Self {
        let filename = format!("{}.csv", Local::today().format("%b-%e-%Y"));

        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .open(path.join(filename))
            .expect("Cannot open or create todays screen time log");

        CurrentState {
            last_frame: None,
            last_line_length: 0usize,
            last_date: Local::today(),
            file: file,
        }
    }
}

fn main() {
    let mut path_buf = dirs::home_dir().unwrap();
    path_buf.push(".screen-time");
    if !path_buf.exists() {
        create_dir(path_buf.as_path()).expect("cannot create .screen-time folder in your HOME");
    }

    let mut very_first_loop = true;

    let mut state = CurrentState::new(&path_buf);

    loop {
        if !very_first_loop {
            //wait for timeout on every consequtive loop cycle
            //it stays on top of the loop so "continue" will also wait for timeout
            std::thread::sleep(std::time::Duration::from_secs(3));
        }
        very_first_loop = false;

        if state.last_date != Local::today() {
            println!("New day! Switching to new file");
            state = CurrentState::new(&path_buf);
        }

        let active_app_name = request_active_app_name();
        if let Err(err) = active_app_name {
            eprintln!("Error reading active app name, {:?}", err);
            continue;
        };
        let active_app_name = active_app_name.unwrap();

        println!("Active app name: {}", active_app_name);

        if should_ignore_app(&active_app_name) {
            continue;
        }

        let frame = compose_frame(&state.last_frame, &active_app_name);

        if frame.end == 0 {
            state.last_frame = Some(frame);
            continue; //user spent less than minimum amount of time here
        }

        let string = frame_to_string(&frame);

        //improve this logic to be more readable
        if let Some(last_frame) = state.last_frame {
            if last_frame.end != 0 {
                if active_app_name == last_frame.name {
                    let _ = state
                        .file
                        .seek(SeekFrom::End(-(state.last_line_length as i64)))
                        .unwrap();
                }
            }
        }

        state.last_frame = Some(frame);
        state.last_line_length = string.len();

        state.file.write(string.as_bytes());
        state.file.sync_data();
    }
}

fn compose_frame(last_frame: &Option<Frame>, name: &str) -> Frame {
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap();
    let timestamp = timestamp.as_secs();
    let mut frame = Frame {
        name: name.to_string(),
        start: timestamp,
        end: 0,
    };

    if let Some(last_frame) = last_frame {
        if last_frame.name == name {
            frame.start = last_frame.start;
            frame.end = timestamp;
        }
    }

    frame
}

fn should_ignore_app(app_name: &str) -> bool {
    if app_name.len() == 1 {
        return true;
    }

    let system_apps = &["Desktop", "unity-panel", "wingpanel"];
    if system_apps.iter().position(|&name| name == app_name).is_some() {
        println!("Ignoring system app");
        return true;
    }

    return false;
}

fn frame_to_string(frame: &Frame) -> String {
    let result = format!(
        "{}{}{}{}{}\n",
        frame.name, DELIM, frame.start, DELIM, frame.end
    );
    return result;
}

fn request_active_app_name() -> Result<String, DaemonError> {
    let output = Command::new("xprop")
        .arg("-root")
        .arg("_NET_ACTIVE_WINDOW")
        .output()
        .expect("Failed to execute xprop. Do you have xprop installed?");
    let output_str = String::from_utf8(output.stdout).map_err(|_| DaemonError::XpropWinIdParse)?;
    let win_id = get_last_word(output_str);

    let output = Command::new("xprop")
        .arg("-id")
        .arg(win_id)
        .arg("WM_CLASS")
        .output()
        .expect("Failed to execute xprop. Do you have xprop installed?");
    let output_str = String::from_utf8(output.stdout).map_err(|_| DaemonError::XpropClassParse)?;

    //wm class line looks like
    //WM_CLASS(STRING) = "chromium-browser", "Chromium-browser"
    //try to extract first parameter here
    //so chromium-browser would app identifier
    let name_start = output_str.find('=');
    let name_end = output_str.find(',');
    if name_start.is_none() || name_end.is_none() {
        return Err(DaemonError::XpropClassParse);
    }
    let name = &output_str[name_start.unwrap() + 3..name_end.unwrap() - 1];
    return Ok(name.to_string());
}

fn get_last_word(string: String) -> String {
    let words = string.split(" ");
    return words.last().unwrap().to_string();
}
