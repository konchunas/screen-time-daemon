use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::time::{Duration, SystemTime};

use std::fs;
use std::fs::{create_dir, File, OpenOptions};

use std::path::PathBuf;
use std::process::Command;

use chrono::{Date, Local, NaiveDate};

static DELIM: &'static str = ";";
static TIMEOUT: u64 = 10;
static DATE_FORMAT: &'static str = "%b-%d-%Y";

#[derive(Debug)]
enum XpropParseError {
    WinId,
    Class,
    DesktopPath,
}

//activity frame
#[derive(Debug)]
pub struct Frame {
    name: String,
    start: u64,
    end: u64,
}

pub enum FrameOperation {
    Prepare(Frame),
    WriteNew(Frame),
    UpdatePrevious(u64),
}

#[derive(Debug)]
struct CurrentState {
    last_frame: Option<Frame>,
    last_write_length: usize,
    last_date: Date<Local>,
    file: File,
    app_info: File,
    app_info_map: HashMap<String, String>,
}

impl CurrentState {
    pub fn new(path: &PathBuf) -> Self {
        let filename = format!("{}.csv", Local::today().format(DATE_FORMAT));

        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .open(path.join(filename))
            .expect("Cannot open or create todays screen time log");

        let mut app_info = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(path.join("app-names.csv"))
            .expect("Cannot open or create app info log");

        let app_info_map = read_desktop_paths(&mut app_info).unwrap();

        CurrentState {
            last_frame: None,
            last_write_length: 0usize,
            last_date: Local::today(),
            file,
            app_info,
            app_info_map,
        }
    }
}

fn main() {
    let mut path_buf = dirs::home_dir().unwrap();
    path_buf.push(".screen-time");
    if !path_buf.exists() {
        create_dir(path_buf.as_path()).expect("Couldn't create .screen-time folder in your HOME");
    } else {
        clean_up_old_logs(&path_buf);
    }

    let mut very_first_loop = true;

    let mut state = CurrentState::new(&path_buf);

    loop {
        if !very_first_loop {
            //wait for timeout on every consequtive loop cycle
            //it stays on top of the loop so "continue" will also wait for timeout
            std::thread::sleep(Duration::from_secs(TIMEOUT));
        }
        very_first_loop = false;

        if state.last_date != Local::today() {
            println!("New day! Switching to new file");
            state = CurrentState::new(&path_buf);
            clean_up_old_logs(&path_buf);
        }

        let active_win_id = match get_active_win_id() {
            Ok(win_id) => win_id,
            Err(err) => {
                eprintln!("Error reading active window ID, {:?}", err);
                state.last_frame = None;
                continue;
            }
        };

        let active_app_name = get_app_name(&active_win_id);
        if let Err(err) = active_app_name {
            eprintln!("Error reading active app name, {:?}", err);
            state.last_frame = None;
            continue;
        };
        let active_app_name = active_app_name.unwrap();

        if should_ignore_app(&active_app_name) {
            println!("Ignoring system app");
            state.last_frame = None;
            continue;
        }

        println!("Active app: {}", active_app_name);

        if !state.app_info_map.contains_key(&active_app_name) {
            let desktop_path = get_desktop_file_path(&active_win_id);
            if let Ok(path) = desktop_path {
                state.app_info_map.insert(active_app_name.clone(), path);
                save_app_info(&state.app_info_map, &mut state.app_info);
            }
        }

        let frame_op = decide(&state.last_frame, &active_app_name);

        let last_frame = match frame_op {
            FrameOperation::Prepare(frame) => frame,
            FrameOperation::WriteNew(frame) => {
                let main_part = format!("{}{}{}{}", frame.name, DELIM, frame.start, DELIM);
                state
                    .file
                    .write_all(main_part.as_bytes())
                    .expect("Error writing to file");
                state.last_write_length = write_timestamp_and_flush(&mut state.file, frame.end);
                frame
            }
            FrameOperation::UpdatePrevious(timestamp) => {
                let last_frame = state.last_frame.unwrap();
                let frame = Frame {
                    name: last_frame.name.clone(),
                    start: last_frame.start,
                    end: timestamp,
                };
                //seek back last timestamp so it can be overwritten
                let _ = state
                    .file
                    .seek(SeekFrom::End(-(state.last_write_length as i64)))
                    .unwrap();
                state.last_write_length = write_timestamp_and_flush(&mut state.file, timestamp);
                frame
            }
        };

        state.last_frame = Some(last_frame);
    }
}

fn write_timestamp_and_flush(file: &mut File, timestamp: u64) -> usize {
    let time_str = format!("{}\n", timestamp);
    let time_str_len = time_str.as_bytes().len();
    file.write_all(time_str.as_bytes()).unwrap_or_else(|_| eprintln!("Couldn't write activity log"));
    file.sync_data().expect("Couldn't flush data to file");
    time_str_len
}

fn decide(last_frame: &Option<Frame>, name: &str) -> FrameOperation {
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap();
    let timestamp = timestamp.as_secs();

    //try to continue previous frame
    if let Some(last_frame) = last_frame {
        if last_frame.name == name {
            if last_frame.end == last_frame.start {
                return FrameOperation::WriteNew(Frame {
                    name: name.to_string(),
                    start: last_frame.start,
                    end: timestamp,
                });
            }
            if timestamp - last_frame.end < TIMEOUT * 5 {
                return FrameOperation::UpdatePrevious(timestamp);
            } else {
                //computer must have been suspended, do not track that as usage
                println!("Too much time passed between this app last logged. Creating new record");
                println!("It was {} seconds", timestamp - last_frame.end);
            }
        }
    }

    //create new frame
    FrameOperation::Prepare(Frame {
        name: name.to_string(),
        start: timestamp,
        end: timestamp,
    })
}

fn should_ignore_app(app_name: &str) -> bool {
    if app_name.len() == 1 {
        return true;
    }

    let system_apps = &["Desktop", "unity-panel", "wingpanel"];
    if system_apps
        .iter()
        .any(|&name| name == app_name)
    {
        println!("Ignoring system app");
        return true;
    }

    false
}

fn get_active_win_id() -> Result<String, XpropParseError> {
    let output = Command::new("xprop")
        .arg("-root")
        .arg("_NET_ACTIVE_WINDOW")
        .output()
        .expect("Failed to execute xprop. Do you have xprop installed?");
    let output_str = String::from_utf8(output.stdout).map_err(|_| XpropParseError::WinId)?;
    output_str
        .split(' ')
        .last()
        .map(|word| word.to_string())
        .ok_or(XpropParseError::WinId)
}

fn get_desktop_file_path(win_id: &str) -> Result<String, XpropParseError> {
    let output = Command::new("xprop")
        .arg("-id")
        .arg(win_id)
        .arg("_BAMF_DESKTOP_FILE")
        .output()
        .expect("Failed to execute xprop. Do you have xprop installed?");
    let output_str =
        String::from_utf8(output.stdout).map_err(|_| XpropParseError::DesktopPath)?;

    let path_start = output_str.find('=');
    let path_end = output_str.len();
    if path_start.is_none() {
        return Err(XpropParseError::DesktopPath);
    }
    let path = &output_str[path_start.unwrap() + 3..path_end - 2];
    Ok(path.to_string())
}

fn get_app_name(win_id: &str) -> Result<String, XpropParseError> {
    let output = Command::new("xprop")
        .arg("-id")
        .arg(win_id)
        .arg("WM_CLASS")
        .output()
        .expect("Failed to execute xprop. Do you have xprop installed?");
    let output_str = String::from_utf8(output.stdout).map_err(|_| XpropParseError::Class)?;

    //wm class line looks like
    //WM_CLASS(STRING) = "chromium-browser", "Chromium-browser"
    //try to extract first parameter here
    //so chromium-browser would app identifier
    let name_start = output_str.find('=');
    let name_end = output_str.find(',');
    if name_start.is_none() || name_end.is_none() {
        return Err(XpropParseError::Class);
    }
    let name = &output_str[name_start.unwrap() + 3..name_end.unwrap() - 1];
    Ok(name.to_string())
}

fn clean_up_old_logs(path: &PathBuf) {
    let last_allowed_date = Local::today() - chrono::Duration::days(14);
    let last_allowed_date = last_allowed_date.naive_local();
    let file_format = format!("{}.csv", DATE_FORMAT);

    for file in std::fs::read_dir(path).unwrap() {
        if let Err(err) = file {
            eprintln!("Cleanup: Error accesing filename of {}", err);
            continue;
        }
        let file = file.unwrap();
        let filename = file.file_name().into_string();
        if let Err(os_str_name) = filename {
            eprintln!("Cleanup: Error reading filename of {:#?}", os_str_name);
            continue;
        }
        let filename = filename.unwrap();
        let date_parse_result = NaiveDate::parse_from_str(&filename, &file_format);
        if let Err(err) = date_parse_result {
            eprintln!(
                "Cleanup: Not removing file {}, it is not log file. Reason: {}",
                filename, err
            );
            continue;
        }
        let file_date = date_parse_result.unwrap();

        if file_date < last_allowed_date {
            if let Err(err) = fs::remove_file(file.path()) {
                eprintln!(
                    "Cleanup: Error removing suitable file {:#?}. Reason {}",
                    file.path(),
                    err
                );
                continue;
            } else {
                println!("Removed old log for {:#?}", file.path());
            }
        }
    }
}

fn read_desktop_paths(file: &mut File) -> std::io::Result<HashMap<String, String>> {
    let mut text = String::new();
    file.read_to_string(&mut text)?;

    let mut map = HashMap::new();
    for line in text.lines() {
        let words: Vec<&str> = line.split(DELIM).collect();
        if words.len() != 2 {
            eprintln!("Skipping line from desktop paths file");
            continue;
        }
        map.insert(words[0].to_string(), words[1].to_string());
    }
    Ok(map)
}

fn save_app_info(map: &HashMap<String, String>, file: &mut File) {
    let _ = file.seek(SeekFrom::Start(0)).unwrap();
    for (key, value) in map {
        let line = format!("{}{}{}\n", key, DELIM, value);
        file.write_all(line.as_bytes()).unwrap_or_else(|_| eprintln!("Couldn't save desktop paths file"));;
    }
}
