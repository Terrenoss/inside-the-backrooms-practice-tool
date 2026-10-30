// Starts the game (as many copies as bot_bouee.cfg asks) and injects float_bot.dll (next to this
// exe) into every running copy that doesn't have it yet, so several bots can search together.
use std::ffi::c_void;
use std::mem::size_of;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use hudhook::windows::core::{s, w, HSTRING};
use hudhook::windows::Win32::Foundation::{CloseHandle, HANDLE};
use hudhook::windows::Win32::System::Diagnostics::Debug::WriteProcessMemory;
use hudhook::windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Module32FirstW, Module32NextW, Process32FirstW, Process32NextW, MODULEENTRY32W,
    PROCESSENTRY32W, TH32CS_SNAPMODULE, TH32CS_SNAPPROCESS,
};
use hudhook::windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use hudhook::windows::Win32::System::Memory::{VirtualAllocEx, VirtualFreeEx, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE};
use hudhook::windows::Win32::System::Threading::{
    CreateRemoteThread, OpenProcess, WaitForSingleObject, INFINITE, PROCESS_ALL_ACCESS,
};

const GAME: &str = "Inside the Backrooms.exe";
const DLL: &str = "float_bot.dll";
const CONFIG: &str = "bot_bouee.cfg";
const DEFAULT_CONFIG: &str = "\
# Bot bouee : reglages (relance bot_bouee.exe apres modification).
# Chemin du jeu.
jeu = D:\\Games\\Inside The Backrooms v0.8.0\\Inside the Backrooms.exe
# Nombre de copies du jeu a faire tourner en meme temps (si le jeu l'accepte).
copies = 1
# Au lancement, le bot charge ta sauvegarde (Continuer) et cherche tout seul jusqu'a reussir : oui / non.
auto = oui
# Essais acceleres : la physique d'un jeu a 60 images/s, aussi vite que le PC le permet, 1 image sur 4 affichee : oui / non.
rapide = oui
# Vitesse du jeu pendant les essais (0.25 a 4).
vitesse = 1
";
/// How long a copy of the game gets to load its code before giving up on it.
const LOAD_TIMEOUT: Duration = Duration::from_secs(180);

/// (game exe, copies) from bot_bouee.cfg, written with defaults the first time.
fn read_config(path: &Path) -> (PathBuf, usize) {
    if !path.exists() {
        let _ = std::fs::write(path, DEFAULT_CONFIG);
    }
    let text = std::fs::read_to_string(path).unwrap_or_else(|_| DEFAULT_CONFIG.into());
    let mut config = (PathBuf::new(), 1);
    for (key, value) in text.lines().filter(|l| !l.trim_start().starts_with('#')).filter_map(|l| l.split_once('=')) {
        match key.trim() {
            "jeu" => config.0 = PathBuf::from(value.trim()),
            "copies" => config.1 = value.trim().parse().unwrap_or(1).clamp(1, 8),
            _ => {},
        }
    }
    config
}

fn wide_str(chars: &[u16]) -> String {
    String::from_utf16_lossy(&chars[..chars.iter().position(|&c| c == 0).unwrap_or(chars.len())])
}

/// Process ids of every running copy of the game.
unsafe fn game_pids() -> Vec<u32> {
    let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return Vec::new() };
    let mut entry = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
    let mut pids = Vec::new();
    let mut ok = Process32FirstW(snap, &mut entry).is_ok();
    while ok {
        if wide_str(&entry.szExeFile).eq_ignore_ascii_case(GAME) {
            pids.push(entry.th32ProcessID);
        }
        ok = Process32NextW(snap, &mut entry).is_ok();
    }
    let _ = CloseHandle(snap);
    pids
}

unsafe fn has_module(pid: u32, name: &str) -> bool {
    let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, pid) else { return false };
    let mut entry = MODULEENTRY32W { dwSize: size_of::<MODULEENTRY32W>() as u32, ..Default::default() };
    let mut found = false;
    let mut ok = Module32FirstW(snap, &mut entry).is_ok();
    while ok && !found {
        found = wide_str(&entry.szModule).eq_ignore_ascii_case(name);
        ok = Module32NextW(snap, &mut entry).is_ok();
    }
    let _ = CloseHandle(snap);
    found
}

/// LoadLibraryW(dll) in the process, from a remote thread.
unsafe fn inject(process: HANDLE, dll: &std::path::Path) -> Result<(), String> {
    let path = HSTRING::from(dll);
    let bytes = (path.len() + 1) * size_of::<u16>();
    let load = GetProcAddress(GetModuleHandleW(w!("Kernel32")).map_err(|e| e.to_string())?, s!("LoadLibraryW"))
        .ok_or("LoadLibraryW introuvable")?;
    let buf = VirtualAllocEx(process, None, bytes, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE);
    if buf.is_null() {
        return Err("VirtualAllocEx a echoue".into());
    }
    WriteProcessMemory(process, buf, path.as_ptr() as *const c_void, bytes, None).map_err(|e| e.to_string())?;
    let start: unsafe extern "system" fn(*mut c_void) -> u32 = std::mem::transmute(load);
    let thread = CreateRemoteThread(process, None, 0, Some(start), Some(buf), 0, None).map_err(|e| e.to_string())?;
    WaitForSingleObject(thread, INFINITE);
    let _ = CloseHandle(thread);
    let _ = VirtualFreeEx(process, buf, 0, MEM_RELEASE);
    Ok(())
}

fn inject_into(pid: u32, dll: &Path) {
    if unsafe { has_module(pid, DLL) } {
        println!("Jeu {pid} : bot deja injecte.");
        return;
    }
    let result = unsafe {
        OpenProcess(PROCESS_ALL_ACCESS, false, pid).map_err(|e| e.to_string()).and_then(|p| {
            let r = inject(p, dll);
            let _ = CloseHandle(p);
            r
        })
    };
    match result {
        Ok(()) => println!("Jeu {pid} : bot injecte, sa fenetre console s'ouvre."),
        Err(e) => println!("Jeu {pid} : injection ratee ({e})."),
    }
}

/// Starts one copy of the game and waits until its code (GameAssembly.dll) is loaded.
fn start_game(exe: &Path) -> Result<u32, String> {
    let dir = exe.parent().unwrap_or(Path::new("."));
    // Not ours: the game must not inherit this console (the bot opens its own).
    let null = std::process::Stdio::null;
    let child = std::process::Command::new(exe)
        .current_dir(dir)
        .stdin(null())
        .stdout(null())
        .stderr(null())
        .spawn()
        .map_err(|e| format!("lancement rate ({e})"))?;
    let pid = child.id();
    let since = Instant::now();
    while since.elapsed() < LOAD_TIMEOUT {
        std::thread::sleep(Duration::from_secs(1));
        if !unsafe { game_pids() }.contains(&pid) {
            return Err("le jeu s'est ferme (il refuse peut-etre d'etre lance plusieurs fois)".into());
        }
        if unsafe { has_module(pid, "GameAssembly.dll") } {
            // Let it finish starting up.
            std::thread::sleep(Duration::from_secs(5));
            return Ok(pid);
        }
    }
    Err("le jeu met trop de temps a demarrer".into())
}

fn main() {
    let here = std::env::current_exe().unwrap();
    let dll = here.with_file_name(DLL);
    let (exe, copies) = read_config(&here.with_file_name(CONFIG));
    if !dll.exists() {
        println!("DLL introuvable : {}", dll.display());
    } else {
        let running = unsafe { game_pids() };
        for &pid in &running {
            inject_into(pid, &dll);
        }
        if running.len() < copies && !exe.exists() {
            println!("Jeu introuvable : {} (corrige 'jeu' dans {CONFIG}).", exe.display());
        } else {
            for i in running.len()..copies {
                println!("Lancement du jeu ({}/{copies})...", i + 1);
                match start_game(&exe) {
                    Ok(pid) => inject_into(pid, &dll),
                    Err(e) => {
                        println!("Copie {} : {e}.", i + 1);
                        break;
                    },
                }
            }
        }
    }
    println!("Appuie sur Entree pour fermer.");
    let _ = std::io::stdin().read_line(&mut String::new());
}
