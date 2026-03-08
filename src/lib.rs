use std::ffi::{CStr, CString};
use std::os::raw::c_int;
use std::path::{Path, PathBuf};
use std::process::Command;
use serde_json::{json, Value};
use std::sync::Mutex;

// Global state to manage FFmpeg path or other resources
// Since this is a dylib, we can use static mutable state with synchronization
lazy_static::lazy_static! {
    static ref FFMPEG_PATH: Mutex<Option<String>> = Mutex::new(None);
}

// We need lazy_static dependency, let's add it to Cargo.toml or use std::sync::OnceLock (Rust 1.70+)
// Assuming Rust 1.70+ is available based on project config (1.70 in Cargo.toml of backend)

// --- FFI Interface ---

#[no_mangle]
pub unsafe extern "C" fn plugin_invoke(
    method: *const u8,
    params: *const u8,
    result_ptr: *mut *mut u8,
) -> c_int {
    let method_str = match CStr::from_ptr(method as *const std::os::raw::c_char).to_str() {
        Ok(s) => s,
        Err(_) => return -1,
    };

    let params_str = match CStr::from_ptr(params as *const std::os::raw::c_char).to_str() {
        Ok(s) => s,
        Err(_) => return -1,
    };

    let params_json: Value = match serde_json::from_str(params_str) {
        Ok(v) => v,
        Err(_) => return -1,
    };

    let result = match method_str {
        "detect" => detect(params_json),
        "extract_metadata" => extract_metadata(params_json),
        "get_stream_url" => get_stream_url(params_json),
        "configure" => configure(params_json),
        "get_decryption_plan" => get_decryption_plan(params_json),
        "get_metadata_read_size" => get_metadata_read_size(params_json),
        _ => Err(format!("Unknown method: {}", method_str)),
    };

    match result {
        Ok(val) => {
            let json = serde_json::to_string(&val).unwrap_or_default();
            let c_string = match CString::new(json) {
                Ok(s) => s,
                Err(_) => return -1,
            };
            *result_ptr = c_string.into_raw() as *mut u8;
            0 // Success
        }
        Err(e) => {
            let error_json = json!({ "error": e }).to_string();
             let c_string = match CString::new(error_json) {
                Ok(s) => s,
                Err(_) => return -1,
            };
            *result_ptr = c_string.into_raw() as *mut u8;
            -1 // Failure
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn plugin_free(ptr: *mut u8) {
    if !ptr.is_null() {
        let _ = CString::from_raw(ptr as *mut std::os::raw::c_char);
    }
}

// --- Implementation ---

fn get_ffmpeg_path() -> String {
    let ffmpeg = FFMPEG_PATH.lock().unwrap();
    if let Some(path) = &*ffmpeg {
        return path.clone();
    }
    
    let mut search_paths = Vec::new();

    // 1. Check relative to Executable (Production usually)
    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(root) = current_exe.parent() {
            search_paths.push(root.to_path_buf());
        }
    }

    // 2. Check relative to CWD (Development usually)
    if let Ok(cwd) = std::env::current_dir() {
        search_paths.push(cwd);
    }

    let exe_ext = std::env::consts::EXE_EXTENSION;

    for root in search_paths {
        // Try to find "plugins" directory
        let possible_plugin_dirs = vec![
            root.join("plugins"),
            root.join("backend").join("plugins"),
            root.join("ting-reader").join("backend").join("plugins"),
            // Case: running from target/debug/deps, so plugins is up 3 levels then plugins
            root.join("..").join("..").join("plugins"), 
        ];

        for plugins_dir in possible_plugin_dirs {
            if plugins_dir.exists() {
                // Look for any folder starting with "FFmpeg Provider" or "ffmpeg-utils"
                if let Ok(entries) = std::fs::read_dir(&plugins_dir) {
                    for entry in entries.filter_map(|e| e.ok()) {
                        let path = entry.path();
                        if path.is_dir() {
                            let dir_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                            if dir_name.starts_with("FFmpeg Provider") || dir_name.starts_with("ffmpeg-utils") {
                                // Found candidate directory, check for binary
                                let mut bin_path = path.join("ffmpeg");
                                if !exe_ext.is_empty() { bin_path.set_extension(exe_ext); }
                                if bin_path.exists() { return bin_path.to_string_lossy().to_string(); }

                                let mut bin_sub_path = path.join("bin").join("ffmpeg");
                                if !exe_ext.is_empty() { bin_sub_path.set_extension(exe_ext); }
                                if bin_sub_path.exists() { return bin_sub_path.to_string_lossy().to_string(); }
                            }
                        }
                    }
                }
            }
        }
    }
    
    // Check ./ffmpeg.exe (CWD root fallback)
    let mut local_path = PathBuf::from("ffmpeg");
    if !exe_ext.is_empty() { local_path.set_extension(exe_ext); }
    if local_path.exists() {
        return local_path.to_string_lossy().to_string();
    }

    // Default to system PATH
    "ffmpeg".to_string()
}

fn get_ffprobe_path() -> String {
    let mut search_paths = Vec::new();

    // 1. Check relative to Executable
    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(root) = current_exe.parent() {
            search_paths.push(root.to_path_buf());
        }
    }

    // 2. Check relative to CWD
    if let Ok(cwd) = std::env::current_dir() {
        search_paths.push(cwd);
    }

    let exe_ext = std::env::consts::EXE_EXTENSION;

    for root in search_paths {
        // Try to find "plugins" directory
        let possible_plugin_dirs = vec![
            root.join("plugins"),
            root.join("backend").join("plugins"),
            root.join("ting-reader").join("backend").join("plugins"),
            root.join("..").join("..").join("plugins"), 
        ];

        for plugins_dir in possible_plugin_dirs {
            if plugins_dir.exists() {
                if let Ok(entries) = std::fs::read_dir(&plugins_dir) {
                    for entry in entries.filter_map(|e| e.ok()) {
                        let path = entry.path();
                        if path.is_dir() {
                            let dir_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                            if dir_name.starts_with("FFmpeg Provider") || dir_name.starts_with("ffmpeg-utils") {
                                let mut bin_path = path.join("ffprobe");
                                if !exe_ext.is_empty() { bin_path.set_extension(exe_ext); }
                                if bin_path.exists() { return bin_path.to_string_lossy().to_string(); }

                                let mut bin_sub_path = path.join("bin").join("ffprobe");
                                if !exe_ext.is_empty() { bin_sub_path.set_extension(exe_ext); }
                                if bin_sub_path.exists() { return bin_sub_path.to_string_lossy().to_string(); }
                            }
                        }
                    }
                }
            }
        }
    }
    
    // Check ./ffprobe.exe (CWD root)
    let mut local_path = PathBuf::from("ffprobe");
    if !exe_ext.is_empty() { local_path.set_extension(exe_ext); }
    if local_path.exists() {
        return local_path.to_string_lossy().to_string();
    }

    "ffprobe".to_string()
}

fn configure(params: Value) -> Result<Value, String> {
    // Allow host to configure ffmpeg path
    if let Some(path) = params.get("ffmpeg_path").and_then(|v| v.as_str()) {
        // Store it globally?
        // For now just acknowledge
        let mut ffmpeg = FFMPEG_PATH.lock().unwrap();
        *ffmpeg = Some(path.to_string());
    }
    Ok(json!({ "status": "configured" }))
}

fn detect(params: Value) -> Result<Value, String> {
    let path_str = params["file_path"].as_str().ok_or("Missing file_path")?;
    let path = Path::new(path_str);
    
    // Check extension
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    let is_supported = ext == "m4a" || ext == "mp4";
    
    // Optionally use ffprobe to verify format
    
    Ok(json!({ "is_supported": is_supported }))
}

fn extract_metadata(params: Value) -> Result<Value, String> {
    let path_str = params["file_path"].as_str().ok_or("Missing file_path")?;
    let ffprobe = get_ffprobe_path();
    
    // Run ffprobe
    // ffprobe -v quiet -print_format json -show_format -show_streams "path"
    let output = Command::new(&ffprobe)
        .arg("-v")
        .arg("quiet")
        .arg("-print_format")
        .arg("json")
        .arg("-show_format")
        // .arg("-show_streams") // Streams info usually not needed for basic metadata but good for duration
        .arg(path_str)
        .output()
        .map_err(|e| format!("Failed to execute ffprobe: {}", e))?;
        
    if !output.status.success() {
        return Err(format!("ffprobe exited with error: {}", String::from_utf8_lossy(&output.stderr)));
    }
    
    let json_out: Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("Failed to parse ffprobe output: {}", e))?;
        
    // Extract fields
    let format = json_out.get("format").ok_or("No format info")?;
    let empty_tags = json!({});
    let tags = format.get("tags").unwrap_or(&empty_tags);
    
    let duration_sec = format.get("duration")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0);
        
    // Map common tags
    // ffprobe usually returns lowercase keys
    let mut metadata = json!({
        "duration": duration_sec,
        "format": "m4a"
    });
    
    let meta_obj = metadata.as_object_mut().unwrap();
    
    if let Some(tags_obj) = tags.as_object() {
        for (k, v) in tags_obj {
            let k_lower = k.to_lowercase();
            let v_str = v.as_str().unwrap_or("").to_string();
            
            match k_lower.as_str() {
                "title" | "nam" | "name" => { meta_obj.insert("title".to_string(), json!(v_str)); },
                "artist" | "art" => { meta_obj.insert("artist".to_string(), json!(v_str)); },
                "album" | "alb" => { meta_obj.insert("album".to_string(), json!(v_str)); },
                "album_artist" | "album artist" | "aart" => { meta_obj.insert("album_artist".to_string(), json!(v_str)); },
                "composer" | "wrt" => { meta_obj.insert("composer".to_string(), json!(v_str)); },
                "date" | "year" | "day" => { meta_obj.insert("year".to_string(), json!(v_str)); },
                "comment" | "cmt" => { meta_obj.insert("comment".to_string(), json!(v_str)); },
                "genre" | "gen" => { meta_obj.insert("genre".to_string(), json!(v_str)); },
                "description" | "desc" | "synopsis" => { meta_obj.insert("description".to_string(), json!(v_str)); },
                _ => {} // Ignore others
            }
        }
    }
    
    Ok(metadata)
}

fn get_stream_url(params: Value) -> Result<Value, String> {
    // This plugin acts as a transcoder.
    // It should return a command that the host can execute to get the stream.
    // OR, if the host expects a URL, we might need to start a local server?
    // Usually Native Plugins for "Format" might just return the stream command if the host supports it.
    
    // However, TingReader's architecture for plugins:
    // If it's a "Format" plugin, it might be called to "get_media_source".
    
    // Let's assume the host asks "how do I play this?".
    // If we want to support transcoding, we might return a special protocol URL or a command.
    
    // If we want to support "streaming" m4a as mp3 (transcoding), we typically do this via a piped command.
    // But the current `audio_streamer.rs` in backend uses `symphonia` or `File`.
    
    // Wait, the user said "support streaming playback (mp3 stream)".
    // This implies the backend will ask the plugin for a stream.
    
    // If the backend calls `get_stream_command`, we can return:
    // ffmpeg -i input.m4a -f mp3 -
    
    let path_str = params["file_path"].as_str().ok_or("Missing file_path")?;
    let ffmpeg = get_ffmpeg_path();
    
    // We construct a command that outputs MP3 data to stdout
    let command = vec![
        ffmpeg,
        "-i".to_string(),
        path_str.to_string(),
        "-f".to_string(),
        "mp3".to_string(),
        "-".to_string()
    ];
    
    Ok(json!({
        "stream_type": "pipe",
        "command": command,
        "content_type": "audio/mpeg"
    }))
}

fn get_decryption_plan(_params: Value) -> Result<Value, String> {
    // Return a plain plan to allow direct streaming
    Ok(json!({
        "segments": [
            {
                "type": "plain",
                "offset": 0,
                "length": -1 // Read until end
            }
        ],
        "total_size": null // Use actual file size
    }))
}

fn get_metadata_read_size(_params: Value) -> Result<Value, String> {
    Ok(json!({
        "size": 1024 * 1024 // 1MB
    }))
}
