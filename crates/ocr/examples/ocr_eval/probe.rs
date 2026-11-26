//! What the process really uses: loaded DLLs, memory, the GPU, and node placement.

use crate::Args;
use ocr::engine::{Engine, Options, Provider};
use ocr::synth::{Fonts, Regime, bench_set};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

/// The file a DLL of this name was loaded from, if it is loaded.
pub fn loaded_module(name: &str) -> Option<String> {
    let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
    let mut path = [0u16; 1024];
    // SAFETY: `wide` is NUL-terminated and `path` is as long as the length passed with it.
    let length = unsafe {
        let module = GetModuleHandleW(wide.as_ptr());
        if module.is_null() {
            return None;
        }
        GetModuleFileNameW(module, path.as_mut_ptr(), path.len() as u32)
    };
    Some(String::from_utf16_lossy(&path[..length as usize]))
}

pub fn peak_working_set_bytes() -> u64 {
    let size = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    // SAFETY: the struct is plain integers, and `size` is its size.
    unsafe {
        let mut counters: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        counters.cb = size;
        if GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, size) == 0 {
            return 0;
        }
        counters.PeakWorkingSetSize as u64
    }
}

const GPU_QUERY: [&str; 2] = [
    "--query-gpu=memory.used,utilization.gpu",
    "--format=csv,noheader,nounits",
];

fn gpu_line(line: &str) -> Option<(u32, u32)> {
    let (memory, load) = line.split_once(',')?;
    Some((memory.trim().parse().ok()?, load.trim().parse().ok()?))
}

/// `(MiB used on the whole GPU, utilisation %)` right now. WDDM gives no per-process figure.
pub fn gpu_now() -> Option<(u32, u32)> {
    let output = Command::new("nvidia-smi").args(GPU_QUERY).output().ok()?;
    gpu_line(String::from_utf8_lossy(&output.stdout).lines().next()?)
}

/// Samples the GPU every 200 ms with a local `nvidia-smi` process.
pub struct GpuSampler {
    child: Child,
    reader: JoinHandle<()>,
    samples: Arc<Mutex<Vec<(u32, u32)>>>,
}

impl GpuSampler {
    pub fn start() -> Option<Self> {
        let mut child = Command::new("nvidia-smi")
            .args(GPU_QUERY)
            .arg("--loop-ms=200")
            .stdout(Stdio::piped())
            .spawn()
            .ok()?;
        let output = child.stdout.take()?;
        let samples = Arc::new(Mutex::new(Vec::new()));
        let shared = samples.clone();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(output).lines().map_while(Result::ok) {
                if let (Some(sample), Ok(mut all)) = (gpu_line(&line), shared.lock()) {
                    all.push(sample);
                }
            }
        });
        Some(Self {
            child,
            reader,
            samples,
        })
    }

    pub fn taken(&self) -> usize {
        self.samples.lock().map_or(0, |s| s.len())
    }

    pub fn finish(mut self) -> Vec<(u32, u32)> {
        // The sampler never exits by itself.
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = self.reader.join();
        self.samples.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

/// Loads every session with ONNX Runtime's verbose log and prints where the graph nodes went.
pub fn nodes(args: &Args) -> Result<(), String> {
    let provider = args.parsed("--provider", Provider::DirectMl { device: 0 })?;
    let messages = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = messages.clone();
    let options = Options {
        provider,
        log: Some(Arc::new(move |_, _, _, location: &str, message: &str| {
            if let Ok(mut all) = sink.lock() {
                all.push(format!("{location}\t{message}"));
            }
        })),
        ..Options::default()
    };
    let engine = Engine::load(&args.models()?, &options)?;
    let set = bench_set(Regime::Fixed, &Fonts::load()?);
    let (lines, _) = engine.read(&set[0].image, |_| true, &mut Vec::new())?;
    println!("{provider}: {} lines read after loading", lines.len());
    let all = messages.lock().map_err(|e| e.to_string())?;
    println!("{} ONNX Runtime log messages; placement report:", all.len());
    let mut model = 0;
    let mut in_report = false;
    for entry in all.iter() {
        let (location, message) = entry.split_once('\t').unwrap_or(("", entry));
        if location.contains("VerifyEachNodeIsAssignedToAnEp") {
            if message.contains("Node placements") {
                model += 1;
                println!("--- session {model} (detector, v6, Korean, in load order)");
                in_report = true;
            }
            if in_report {
                println!("{message}");
            }
        }
    }
    for entry in all.iter().filter(|m| {
        let lower = m.to_lowercase();
        lower.contains("fallback") || lower.contains("falling back") || lower.contains("telemetry")
    }) {
        println!("log: {entry}");
    }
    for dll in ["DirectML.dll", "onnxruntime.dll", "d3d12.dll"] {
        println!(
            "{dll}: {}",
            loaded_module(dll).unwrap_or_else(|| "not loaded".into())
        );
    }
    Ok(())
}
