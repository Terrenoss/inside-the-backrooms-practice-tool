// Float bot: injected into Inside the Backrooms (one or several running copies), it replays the
// Poolrooms float boost from a starting spot: run at the arch, drop the float into yourself, jump on
// it, take it back, again, and a last drop while crossing the ceiling, with the game's own drop,
// pickup, jump, stick and sprint. It varies its run-up, facing and timings, keeps its best tries and
// refines them, to get both the player and the float onto the roof. Driven from its own console
// window; the bots of several game copies share their tries through bot_bouee.txt next to the DLL.
mod game;
#[allow(dead_code)] // shared with the practice tool's copy
mod il2cpp;
mod search;

use std::ffi::c_void;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::sync::{LazyLock, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use hudhook::mh::{MhHook, MH_ApplyQueued, MH_Initialize};
use hudhook::windows::core::w;
use std::os::windows::io::IntoRawHandle;

use hudhook::windows::Win32::Foundation::{HANDLE, HINSTANCE, HMODULE};
use hudhook::windows::Win32::System::Console::{
    AllocConsole, GetConsoleWindow, SetConsoleTitleW, SetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
};
use hudhook::windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use hudhook::windows::Win32::UI::WindowsAndMessaging::{DeleteMenu, GetSystemMenu, MF_BYCOMMAND, SC_CLOSE};

use game::{yaw_axes, yaw_of, yaw_rotation, G, V3};
use il2cpp::{alive, rd, wr, P};
use search::{from_line, to_line, End, FloatEnd, Outcome, Params, Search};

static DIR: OnceLock<PathBuf> = OnceLock::new();
static GAME: OnceLock<G> = OnceLock::new();
static BOT: LazyLock<Mutex<Bot>> = LazyLock::new(|| Mutex::new(Bot::new()));
static COMMANDS: Mutex<Vec<Cmd>> = Mutex::new(Vec::new());
static O_UPDATE: AtomicUsize = AtomicUsize::new(0);
static O_MENU_UPDATE: AtomicUsize = AtomicUsize::new(0);

/// Farthest the float can be to take it back (an estimate of the game's interaction reach).
const REACH: f32 = 2.5;
/// Game seconds: on the ground after being put back, at most before going anyway, the longest
/// run-up, how long a jump waits for the player to land on the float, the longest look at where
/// things end up after the boosts, and at most for a whole try.
const SETTLE: f32 = 0.15;
const SETTLE_MAX: f32 = 0.6;
const RUNUP_MAX: f32 = 3.0;
const JUMP_WAIT: f32 = 0.3;
const MEASURE: f32 = 2.0;
const TIMEOUT: f32 = 15.0;
/// Fast mode: game seconds per frame (the physics of 60 FPS), and one frame drawn out of this many.
const TURBO_STEP: f32 = 1.0 / 60.0;
const TURBO_DRAW_EVERY: i32 = 4;
/// Tries between two speed reports.
const REPORT_EVERY: u32 = 20;
/// Longest recording of the player's own run, in game seconds.
const RECORD_MAX: f32 = 30.0;
/// Automatic mode: game seconds in a level before setting up (it streams in), and between two
/// Continue clicks in the main menu.
const LEVEL_SETTLE: f32 = 3.0;
const MENU_RETRY: Duration = Duration::from_secs(30);
const SHARED: &str = "bot_bouee.txt";
const CONFIG: &str = "bot_bouee.cfg";

const HELP: &str = "\
Commandes :
  depart        ta position, ton orientation et la bouee en main deviennent le depart
                (au pied de l'arche, ou sur l'arche pour ne chercher que la traversee du toit)
  rejoindre     prend le depart d'un bot lance dans un autre jeu (meme niveau)
  lancer [n]    lance n essais (200 par defaut)
  stop          arrete
  rapide oui|non  essais acceleres (physique a 60 images/s, peu d'images affichees)
  vitesse x     vitesse du jeu pendant les essais (0.25 a 4, 1 par defaut)
  meilleurs     les 10 meilleurs essais
  rejouer i     rejoue le i-eme meilleur, pour le regarder
  enregistre    enregistre ta propre montee (retape pour finir) dans un fichier demo_*.txt
  auto oui|non  mode automatique : charge la sauvegarde et cherche jusqu'a reussir
  aide          cette aide";

/// To the bot's console. Never panics (println! does when stdout is gone), even from a game hook.
fn say(msg: &str) {
    let mut out = std::io::stdout();
    let _ = writeln!(out, "{msg}");
    let _ = out.flush();
}

#[derive(Debug, PartialEq)]
enum Cmd {
    Depart,
    Rejoindre,
    Lancer(u32),
    Stop,
    Rapide(bool),
    Vitesse(f32),
    Meilleurs,
    Rejouer(usize),
    Enregistre,
    Auto(bool),
    Aide,
}

fn yes(word: &str) -> Option<bool> {
    match word.to_lowercase().as_str() {
        "oui" | "on" | "1" | "true" => Some(true),
        "non" | "off" | "0" | "false" => Some(false),
        _ => None,
    }
}

struct Config {
    auto: bool,
    speed: f32,
    fast: bool,
}

/// bot_bouee.cfg next to the DLL (shared with the launcher): `auto = oui`, `vitesse = 1`, `rapide = oui`.
fn read_config() -> Config {
    let text = DIR.get().and_then(|d| std::fs::read_to_string(d.join(CONFIG)).ok()).unwrap_or_default();
    let mut config = Config { auto: true, speed: 1.0, fast: true };
    for (key, value) in text.lines().filter(|l| !l.trim_start().starts_with('#')).filter_map(|l| l.split_once('=')) {
        let value = value.trim();
        match key.trim() {
            "auto" => config.auto = yes(value).unwrap_or(config.auto),
            "rapide" => config.fast = yes(value).unwrap_or(config.fast),
            "vitesse" => config.speed = value.replace(',', ".").parse::<f32>().map_or(config.speed, |v| v.clamp(0.25, 4.0)),
            _ => {},
        }
    }
    config
}

fn parse(line: &str) -> Result<Cmd, String> {
    let mut words = line.split_whitespace();
    let word = words.next().unwrap_or("").to_lowercase();
    let arg = words.next();
    let number = |default: Option<f32>| match (arg.map(|a| a.replace(',', ".").parse::<f32>()), default) {
        (Some(Ok(v)), _) => Ok(v),
        (None, Some(d)) => Ok(d),
        _ => Err(format!("'{word}' attend un nombre.")),
    };
    let on_off = || arg.map_or(Some(true), yes).ok_or(format!("'{word}' attend oui ou non."));
    Ok(match word.as_str() {
        "depart" | "départ" => Cmd::Depart,
        "rejoindre" => Cmd::Rejoindre,
        "lancer" => Cmd::Lancer(number(Some(200.0))?.max(1.0) as u32),
        "stop" => Cmd::Stop,
        "rapide" => Cmd::Rapide(on_off()?),
        "vitesse" => Cmd::Vitesse(number(None)?.clamp(0.25, 4.0)),
        "meilleurs" => Cmd::Meilleurs,
        "rejouer" => Cmd::Rejouer(number(Some(1.0))?.max(1.0) as usize),
        "enregistre" | "enregistrer" => Cmd::Enregistre,
        "auto" => Cmd::Auto(on_off()?),
        "aide" | "help" | "" => Cmd::Aide,
        _ => return Err(format!("Commande inconnue : '{word}'. Tape 'aide'.")),
    })
}

/// Where tries start: a spot, a facing, and which float (by name, the same in every copy of the game).
#[derive(Clone)]
struct Start {
    pos: V3,
    yaw: f32,
    float_name: String,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Phase {
    Reset,
    Settle,
    RunUp,
    Drop,
    Jump,
    Grab,
    Next,
    Measure,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    /// A try of the search.
    Try,
    /// A success of the fast mode, replayed at normal speed before believing it.
    Verify,
    /// One of the best, replayed to be watched.
    Replay,
}

#[derive(Clone, Copy)]
struct Run {
    params: Params,
    kind: Kind,
    phase: Phase,
    /// Game seconds in this phase, and since the try began.
    t: f32,
    total: f32,
    boosts: u32,
    end: Option<End>,
    /// The start moved to its side, the run-up there was room for, and what the player collides with.
    base: V3,
    runup: f32,
    mask: i32,
    start_y: f32,
    best_y: f32,
    last_y: f32,
    ceiling: Option<f32>,
    /// Where the float was right after the last drop, and last frame.
    spawn: Option<V3>,
    float_last: Option<V3>,
    /// Game seconds the player and the float have stopped moving.
    still: f32,
    /// Took the float back from the roof.
    regrabbed: bool,
}

impl Run {
    fn new(params: Params, kind: Kind) -> Run {
        Run {
            params,
            kind,
            phase: Phase::Reset,
            t: 0.0,
            total: 0.0,
            boosts: 0,
            end: None,
            base: V3::default(),
            runup: 0.0,
            mask: 0,
            start_y: 0.0,
            best_y: 0.0,
            last_y: 0.0,
            ceiling: None,
            spawn: None,
            float_last: None,
            still: 0.0,
            regrabbed: false,
        }
    }

    fn go(&mut self, phase: Phase) {
        (self.phase, self.t) = (phase, 0.0);
    }

    /// What the bot holds: the move stick (right, forward) and the sprint. Full stick in the run-up,
    /// the try's own amount while boosting, nothing once done.
    fn stick(&self) -> ([f32; 2], bool) {
        let p = self.params;
        match self.phase {
            Phase::RunUp => (p.stick(1.0), p.sprinting()),
            Phase::Drop | Phase::Jump | Phase::Grab | Phase::Next => (p.stick(p.amount), p.sprinting()),
            _ => ([0.0; 2], false),
        }
    }
}

/// Frame settings changed by the fast mode, to put back.
struct Saved {
    fps: i32,
    vsync: i32,
    interval: i32,
}

/// The player's own run, frame by frame.
struct Recording {
    float: usize,
    t: f32,
    carrying: bool,
    drops: u32,
    grabs: u32,
    lines: Vec<String>,
}

struct Bot {
    search: Search,
    start: Option<Start>,
    /// This game's float for the start (its PoolroomsFloaterCarry).
    float: usize,
    run: Option<Run>,
    running: bool,
    wanted: u32,
    done: u32,
    speed: f32,
    user_time_scale: Option<f32>,
    /// Fast mode wanted, and on (with what it changed).
    fast: bool,
    turbo: Option<Saved>,
    /// Since when, game seconds and tries for the next speed report.
    report: (Instant, f32, u32),
    /// Let go of the stick and the sprint (once, after a try).
    release: bool,
    recording: Option<Recording>,
    shared_read: usize,
    last_sync: Option<Instant>,
    /// Local player controller: a new one means a new level.
    ctrl: usize,
    /// Automatic mode: continue the saved game from the main menu, set up in the level and search
    /// until a try works. Game seconds in the current level, whether it set up in it, when it last
    /// clicked Continue, and whether it won.
    auto: bool,
    in_level: f32,
    set_up: bool,
    /// Already said this level has no float for the start.
    warned: bool,
    continued: Option<Instant>,
    won: bool,
}

/// Differs per game copy and per run, so bots don't all try the same things.
fn seed() -> u64 {
    std::process::id() as u64 * 0x2545_F491_4F6C_DD1D ^ unix_time().as_nanos() as u64
}

fn unix_time() -> Duration {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default()
}

impl Bot {
    fn new() -> Bot {
        Bot {
            search: Search::new(seed()),
            start: None,
            float: 0,
            run: None,
            running: false,
            wanted: 0,
            done: 0,
            speed: 1.0,
            user_time_scale: None,
            fast: true,
            turbo: None,
            report: (Instant::now(), 0.0, 0),
            release: false,
            recording: None,
            shared_read: 0,
            last_sync: None,
            ctrl: 0,
            auto: false,
            in_level: 0.0,
            set_up: false,
            warned: false,
            continued: None,
            won: false,
        }
    }
}

fn shared_path() -> Option<PathBuf> {
    DIR.get().map(|d| d.join(SHARED))
}

/// The start written by the bot that ran `depart`.
fn shared_start() -> Option<Start> {
    let text = std::fs::read_to_string(shared_path()?).ok()?;
    let rest = text.lines().next()?.strip_prefix("START ")?;
    let mut f = rest.splitn(5, ' ');
    let mut num = || f.next()?.parse::<f32>().ok();
    let (x, y, z, yaw) = (num()?, num()?, num()?, num()?);
    Some(Start { pos: V3 { x, y, z }, yaw, float_name: f.next()?.trim().to_string() })
}

fn append_shared(line: &str) {
    if let Some(Ok(mut f)) = shared_path().map(|p| std::fs::OpenOptions::new().create(true).append(true).open(p)) {
        let _ = writeln!(f, "{line}");
    }
}

/// Tries from the other games' bots, merged into ours.
fn sync_shared(bot: &mut Bot) {
    bot.last_sync = Some(Instant::now());
    let Some(text) = shared_path().and_then(|p| std::fs::read_to_string(p).ok()) else { return };
    let lines: Vec<&str> = text.lines().collect();
    let me = std::process::id();
    let mut new = 0;
    for o in lines.iter().skip(bot.shared_read).filter_map(|l| from_line(l, me)) {
        bot.search.add(o);
        new += 1;
    }
    bot.shared_read = lines.len();
    if new > 0 && bot.running {
        say(&format!("  (+{new} essais des autres jeux)"));
    }
}

unsafe fn set_time_scale(g: &G, v: f32) {
    call!(g.time_scale_set, fn(f32), v);
}

/// Fast mode on or off (nothing if already so).
unsafe fn set_turbo(g: &G, bot: &mut Bot, on: bool) {
    let Some(t) = &g.turbo else { return };
    match (on, bot.turbo.is_some()) {
        (true, false) => {
            bot.turbo = Some(Saved {
                fps: call!(t.fps_get, fn() -> i32),
                vsync: t.vsync_get.map_or(1, |f| f()),
                interval: call!(t.interval_get, fn() -> i32),
            });
            call!(t.vsync_set, fn(i32), 0);
            call!(t.fps_set, fn(i32), -1);
            call!(t.capture_dt, fn(f32), TURBO_STEP);
            il2cpp::static_set(t.interval, TURBO_DRAW_EVERY);
        },
        (false, true) => {
            let saved = bot.turbo.take().unwrap();
            call!(t.capture_dt, fn(f32), 0.0);
            il2cpp::static_set(t.interval, saved.interval.max(1));
            call!(t.fps_set, fn(i32), saved.fps);
            call!(t.vsync_set, fn(i32), saved.vsync);
        },
        _ => {},
    }
}

unsafe fn stop(g: &G, bot: &mut Bot, why: &str) {
    set_turbo(g, bot, false);
    if let Some(v) = bot.user_time_scale.take() {
        set_time_scale(g, v);
    }
    (bot.running, bot.run, bot.release) = (false, None, true);
    say(why);
}

/// The float the local player holds, if any.
unsafe fn held_float(g: &G, ctrl: P) -> Option<P> {
    let (_, me) = g.my_net_id(ctrl);
    (me != 0).then(|| g.floats().into_iter().find(|&f| alive(f) && g.carrier(f) == me)).flatten()
}

/// This level's float for a start: the one of the same name nearest to it.
unsafe fn float_for(g: &G, start: &Start) -> Option<P> {
    g.floats()
        .into_iter()
        .filter(|&f| alive(f) && g.name(f) == start.float_name)
        .min_by(|&a, &b| g.pos(a).dist(start.pos).total_cmp(&g.pos(b).dist(start.pos)))
}

/// Starts `n` tries (u32::MAX: until one works, in automatic mode).
unsafe fn begin(g: &G, bot: &mut Bot, n: u32) {
    (bot.running, bot.wanted, bot.done) = (true, n, 0);
    bot.report = (Instant::now(), 0.0, 0);
    if bot.user_time_scale.is_none() {
        bot.user_time_scale = Some(call!(g.time_scale_get, fn() -> f32));
    }
    set_time_scale(g, bot.speed);
    // Keep going while another game window has the focus.
    call!(g.run_in_background, fn(bool), true);
    if bot.fast && g.turbo.is_none() {
        say("Mode rapide indisponible dans cette version du jeu : vitesse normale.");
    }
}

/// Automatic mode, once in a level: the start (written by `depart`, kept across launches), its float
/// in this level, and searching until a try works.
unsafe fn auto_setup(g: &G, bot: &mut Bot) {
    bot.in_level = 0.0;
    let Some(start) = bot.start.clone().or_else(shared_start) else {
        say("Mode auto : pas encore de depart. Mets-toi au depart avec la licorne en main et tape 'depart'.");
        bot.set_up = true;
        return;
    };
    let Some(float) = float_for(g, &start) else {
        if !bot.warned {
            say(&format!("Mode auto : pas de bouee '{}' ici (pas dans les Poolrooms ?). J'attends.", start.float_name));
            bot.warned = true;
        }
        return;
    };
    (bot.start, bot.float, bot.set_up, bot.shared_read) = (Some(start), float as usize, true, 0);
    // Tries from earlier sessions and other games are in the shared file: start from them.
    sync_shared(bot);
    begin(g, bot, u32::MAX);
    say(&format!("Mode auto : je cherche jusqu'a reussir ({} essais deja connus).", bot.search.results.len()));
}

unsafe fn command(g: &G, bot: &mut Bot, ctrl: P, cmd: Cmd) {
    match cmd {
        Cmd::Aide => say(HELP),
        Cmd::Depart => {
            if bot.running || bot.run.is_some() {
                say("Le bot tourne : tape 'stop' d'abord, mets-toi en place, puis 'depart'.");
                return;
            }
            let Some(float) = held_float(g, ctrl) else {
                say("Prends la bouee en main d'abord, puis tape 'depart'.");
                return;
            };
            let tr = call!(g.comp_transform, fn(P) -> P, ctrl);
            let start = Start {
                pos: call!(g.tr_get_pos, fn(P) -> V3, tr),
                yaw: yaw_of(call!(g.tr_get_rot, fn(P) -> game::Quat, tr)),
                float_name: g.name(float),
            };
            if let Some(p) = shared_path() {
                let s = &start;
                let _ = std::fs::write(p, format!("START {} {} {} {} {}\n", s.pos.x, s.pos.y, s.pos.z, s.yaw, s.float_name));
            }
            say(&format!(
                "Depart : {:.2} {:.2} {:.2}, orientation {:.0}, bouee '{}'.",
                start.pos.x, start.pos.y, start.pos.z, start.yaw, start.float_name
            ));
            // A new start: earlier tries don't apply, and the shared file now only holds START.
            (bot.start, bot.float, bot.search, bot.shared_read) = (Some(start), float as usize, Search::new(seed()), 1);
            (bot.set_up, bot.won) = (true, false);
            if bot.auto {
                begin(g, bot, u32::MAX);
                say("Mode auto : je cherche jusqu'a reussir. Ne touche plus au jeu.");
            } else {
                say("Tape 'lancer' (ou 'auto' pour chercher jusqu'a reussir).");
            }
        },
        Cmd::Rejoindre => {
            let Some(start) = shared_start() else {
                say("Aucun depart partage : tape 'depart' dans le bot d'un autre jeu d'abord.");
                return;
            };
            let Some(float) = float_for(g, &start) else {
                say(&format!("Pas de bouee '{}' dans ce niveau.", start.float_name));
                return;
            };
            say(&format!("Rejoint : depart {:.2} {:.2} {:.2}, bouee '{}'. Tape 'lancer'.", start.pos.x, start.pos.y, start.pos.z, start.float_name));
            (bot.start, bot.float, bot.shared_read) = (Some(start), float as usize, 0);
            sync_shared(bot);
        },
        Cmd::Lancer(n) => {
            if bot.start.is_none() {
                say("Pas de depart : tape 'depart' (bouee en main) ou 'rejoindre'.");
                return;
            }
            begin(g, bot, n);
            say(&format!("Lance : {n} essais, vitesse {}x{}. Ne touche plus au jeu.", bot.speed, if bot.fast { ", rapide" } else { "" }));
        },
        Cmd::Stop => {
            // Stopping by hand also stops the automatic mode for this session.
            bot.auto = false;
            stop(g, bot, "Arrete (mode auto coupe, 'auto' pour le relancer).");
        },
        Cmd::Auto(on) => {
            (bot.auto, bot.won, bot.set_up, bot.warned, bot.in_level) = (on, false, false, false, 0.0);
            if !on && (bot.running || bot.run.is_some()) {
                stop(g, bot, "Mode auto coupe.");
            } else {
                say(if on { "Mode auto : je m'installe dans quelques secondes." } else { "Mode auto coupe." });
            }
        },
        Cmd::Rapide(on) => {
            bot.fast = on;
            say(if on { "Mode rapide : oui." } else { "Mode rapide : non (essais a vitesse normale)." });
        },
        Cmd::Vitesse(v) => {
            bot.speed = v;
            if bot.running {
                set_time_scale(g, v);
            }
            say(&format!("Vitesse {v}x."));
        },
        Cmd::Meilleurs => {
            if bot.search.results.is_empty() {
                say("Aucun essai pour l'instant.");
            }
            for (i, o) in bot.search.results.iter().take(10).enumerate() {
                say(&format!("{:2}. {}\n    {}", i + 1, o.summary(), o.params.text()));
            }
        },
        Cmd::Rejouer(i) => match (bot.search.results.get(i - 1), &bot.start) {
            _ if bot.running => say("Le bot cherche : tape 'stop' d'abord."),
            (Some(o), Some(_)) => {
                say(&format!("Rejoue {i} : {}", o.params.text()));
                bot.run = Some(Run::new(o.params, Kind::Replay));
            },
            _ => say("Rien a rejouer (tape 'meilleurs')."),
        },
        Cmd::Enregistre => {
            if bot.recording.is_some() {
                save_recording(bot);
            } else if bot.running || bot.run.is_some() {
                say("Le bot tourne : tape 'stop' d'abord.");
            } else {
                let float = held_float(g, ctrl).or_else(|| bot.start.as_ref().and_then(|s| float_for(g, s)));
                let carrying = held_float(g, ctrl).is_some();
                bot.recording =
                    Some(Recording { float: float.map_or(0, |f| f as usize), t: 0.0, carrying, drops: 0, grabs: 0, lines: Vec::new() });
                say("J'enregistre ta montee (30 s au plus). Retape 'enregistre' pour finir.");
            }
        },
    }
}

/// One frame of the player's own run: where they are and what they press.
unsafe fn record(g: &G, bot: &mut Bot, ctrl: P) {
    if bot.run.is_some() {
        return;
    }
    let Some(rec) = bot.recording.as_mut() else { return };
    rec.t += call!(g.delta_time, fn() -> f32);
    let float = rec.float as P;
    let float_alive = !float.is_null() && alive(float);
    let (_, me) = g.my_net_id(ctrl);
    let carrying = float_alive && me != 0 && g.carrier(float) == me;
    if carrying != rec.carrying {
        rec.lines.push(format!("# {} {:.4}", if carrying { "E" } else { "Q" }, rec.t));
        if carrying {
            rec.grabs += 1;
        } else {
            rec.drops += 1;
        }
        rec.carrying = carrying;
    }
    let tr = call!(g.comp_transform, fn(P) -> P, ctrl);
    let (feet, yaw) = (call!(g.tr_get_pos, fn(P) -> V3, tr), yaw_of(call!(g.tr_get_rot, fn(P) -> game::Quat, tr)));
    let stick: [f32; 2] = rd(ctrl, g.off_move_dir);
    let f = if float_alive { g.pos(float) } else { V3::default() };
    rec.lines.push(format!(
        "{:.4} {:.3} {:.3} {:.3} {:.1} {:.2} {:.2} {} {} {} {:.3} {:.3} {:.3} {:.3}",
        rec.t,
        feet.x,
        feet.y,
        feet.z,
        yaw,
        stick[0],
        stick[1],
        rd::<bool>(ctrl, g.off_run_hold) as i32,
        g.grounded(ctrl) as i32,
        carrying as i32,
        rd::<f32>(ctrl, g.off_gravity),
        f.x,
        f.y,
        f.z
    ));
    if rec.t >= RECORD_MAX {
        save_recording(bot);
    }
}

fn save_recording(bot: &mut Bot) {
    let Some(rec) = bot.recording.take() else { return };
    let name = format!("demo_{}.txt", unix_time().as_secs());
    let mut text = String::from("# t x y z orientation stick_droite stick_avant sprint au_sol en_main gravite bouee_x bouee_y bouee_z\n");
    if let Some(s) = &bot.start {
        text += &format!("# depart {} {} {} {}\n", s.pos.x, s.pos.y, s.pos.z, s.yaw);
    }
    for line in &rec.lines {
        text += line;
        text.push('\n');
    }
    let saved = DIR.get().map(|d| std::fs::write(d.join(&name), text));
    match saved {
        Some(Ok(())) => say(&format!("Enregistre : {:.1} s, {} Q, {} E -> {name}", rec.t, rec.drops, rec.grabs)),
        _ => say("Impossible d'ecrire l'enregistrement."),
    }
}

unsafe fn tick(g: &G, bot: &mut Bot, ctrl: P) {
    if bot.ctrl != ctrl as usize {
        if bot.running || bot.run.is_some() {
            stop(g, bot, "Changement de niveau : arrete.");
        }
        if bot.recording.is_some() {
            save_recording(bot);
        }
        (bot.ctrl, bot.float, bot.in_level, bot.set_up, bot.warned, bot.release) = (ctrl as usize, 0, 0.0, false, false, false);
        if let Some(start) = bot.start.clone() {
            bot.float = float_for(g, &start).map_or(0, |f| f as usize);
        }
    }
    let commands = std::mem::take(&mut *COMMANDS.lock().unwrap_or_else(|e| e.into_inner()));
    for cmd in commands {
        command(g, bot, ctrl, cmd);
    }
    if bot.auto && !bot.won && !bot.set_up && !bot.running && bot.run.is_none() {
        bot.in_level += call!(g.delta_time, fn() -> f32);
        if bot.in_level >= LEVEL_SETTLE {
            auto_setup(g, bot);
        }
    }
    if bot.running && bot.last_sync.is_none_or(|t| t.elapsed() > Duration::from_secs(3)) {
        sync_shared(bot);
    }
    record(g, bot, ctrl);
    step(g, bot, ctrl);
    // Fast only for the search's own tries: checks and replays run at normal speed.
    let fast = bot.fast && bot.running && bot.run.is_none_or(|r| r.kind == Kind::Try);
    set_turbo(g, bot, fast);
}

/// One frame of the current try: put back behind the start, run at the arch, drop / jump / take
/// back, again, the last drop while crossing the ceiling, and a look at where the player and the
/// float end up.
unsafe fn step(g: &G, bot: &mut Bot, ctrl: P) {
    let Some(start) = bot.start.clone() else { return };
    let Some(mut run) = bot.run.take() else {
        if bot.running && bot.done < bot.wanted {
            bot.run = Some(Run::new(bot.search.next_params(), Kind::Try));
        } else if bot.running {
            let best = bot.search.results.first().map_or(String::new(), |o| format!("\n  {}\n  {}", o.summary(), o.params.text()));
            stop(g, bot, &format!("Fini ({} essais). Meilleur :{best}", bot.done));
        }
        return;
    };
    let p = run.params;
    let float = bot.float as P;
    let float_alive = !float.is_null() && alive(float);
    let (identity, me) = g.my_net_id(ctrl);
    let carrying = float_alive && me != 0 && g.carrier(float) == me;
    let feet = g.pos(ctrl);
    let dt = call!(g.delta_time, fn() -> f32);
    (run.t, run.total) = (run.t + dt, run.total + dt);
    run.best_y = run.best_y.max(feet.y);
    let last_y = std::mem::replace(&mut run.last_y, feet.y);
    // The run's line, and where the player looks.
    let (fwd, right) = yaw_axes(start.yaw + p.heading);
    let yaw = start.yaw + p.yaw;
    // What Q and E do (as host, the commands' server-side bodies).
    let release = || call!(g.carry_drop, fn(P, P), float, identity);
    let take_back = || call!(g.carry_pickup, fn(P, P), float, identity);

    // The last drop: float in hand while the feet cross the ceiling.
    let climbing = matches!(run.phase, Phase::Drop | Phase::Jump | Phase::Grab | Phase::Next);
    if climbing && carrying && run.ceiling.is_some_and(|c| feet.y >= c + p.last_drop) {
        release();
        run.end = Some(End::LastDrop);
        run.go(Phase::Measure);
    } else if run.total > TIMEOUT && run.phase != Phase::Measure {
        run.end.get_or_insert(End::Timeout);
        run.go(Phase::Measure);
    }
    match run.phase {
        Phase::Reset => {
            if !float_alive {
                stop(g, bot, "La bouee du depart n'existe plus dans ce niveau.");
                return;
            }
            let player_layer = g.layer(ctrl);
            run.mask = g.mask(player_layer, &[player_layer, g.layer(float)]);
            let beside = start.pos.add_scaled(right, p.side);
            run.base = if g.floor_near(beside, run.mask) { beside } else { start.pos };
            run.runup = g.runup_room(run.base, fwd, p.runup, run.mask);
            g.teleport(ctrl, run.base.add_scaled(fwd, -run.runup), yaw_rotation(yaw));
            wr(ctrl, g.off_gravity, 0.0f32);
            if !carrying && alive(identity) {
                take_back();
            }
            g.refill_stamina();
            run.go(Phase::Settle);
        },
        Phase::Settle if (run.t >= SETTLE && g.grounded(ctrl)) || run.t >= SETTLE_MAX => {
            if !carrying {
                stop(g, bot, "La bouee ne revient pas en main : arrete.");
                return;
            }
            run.ceiling = g.ray(start.pos.add_scaled(V3::UP, 0.5), V3::UP, 10.0, run.mask).map(|d| start.pos.y + 0.5 + d);
            (run.start_y, run.best_y) = (start.pos.y, feet.y);
            run.go(Phase::RunUp);
        },
        Phase::RunUp => {
            if feet.sub(run.base).dot(fwd) >= p.first_drop || run.t >= RUNUP_MAX {
                run.go(Phase::Drop);
            }
        },
        Phase::Drop => {
            if carrying {
                release();
                run.boosts += 1;
                run.go(Phase::Jump);
            } else {
                run.end = Some(End::NotHeld);
                run.go(Phase::Measure);
            }
        },
        // On the float (or waited long enough for it).
        Phase::Jump if run.t >= p.jump_after && (g.grounded(ctrl) || run.t >= p.jump_after + JUMP_WAIT) => {
            call!(g.start_jump, fn(P), ctrl);
            run.go(Phase::Grab);
        },
        Phase::Grab if run.t >= p.grab_after => {
            let cam = call!(g.player_camera, fn() -> P);
            let eye = if alive(cam) { g.pos(cam) } else { feet };
            if !carrying && float_alive && g.pos(float).dist(eye) <= REACH {
                take_back();
                run.go(Phase::Next);
            } else {
                run.end = Some(End::TooFar);
                run.go(Phase::Measure);
            }
        },
        Phase::Next if run.t >= p.next_after => {
            if run.boosts >= p.cycles {
                if carrying {
                    release();
                }
                run.end = Some(End::Cycles);
                run.go(Phase::Measure);
            } else {
                run.go(Phase::Drop);
            }
        },
        Phase::Measure => {
            if run.spawn.is_none() && !carrying && float_alive {
                run.spawn = Some(g.pos(float));
            }
            // On the roof with the float stuck in it: take it back if it sticks out of the roof
            // (a player can only aim at what shows), like saopy does.
            if let (Some(c), false, true) = (run.ceiling, carrying, float_alive) {
                if !run.regrabbed && feet.y >= c && feet.y >= g.roof_top(feet, c, run.mask) - 0.1 {
                    let at = g.pos(float);
                    let shows = g.float_top(float).is_some_and(|top| top > g.roof_top(at, c, run.mask) + 0.05);
                    let cam = call!(g.player_camera, fn() -> P);
                    let eye = if alive(cam) { g.pos(cam) } else { feet };
                    if shows && at.dist(eye) <= REACH {
                        take_back();
                        run.regrabbed = true;
                    }
                }
            }
            let at = (float_alive && !carrying).then(|| g.pos(float));
            let float_moved = at.zip(run.float_last).map_or(0.0, |(a, b)| a.dist(b));
            run.float_last = at;
            let resting = (feet.y - last_y).abs() < 0.002 && float_moved < 0.003;
            run.still = if resting { run.still + dt } else { 0.0 };
            // Falling back under the ceiling: it can't end on the roof any more.
            let falling_back = run.t >= 0.25 && feet.y < last_y - 0.002 && run.ceiling.is_none_or(|c| feet.y < c - 0.2);
            if run.t >= MEASURE || falling_back || (run.t >= 0.5 && run.still >= 0.25) {
                finish(g, bot, &run, feet, float, float_alive, carrying);
                return;
            }
        },
        _ => {},
    }
    bot.run = Some(run);
}

unsafe fn finish(g: &G, bot: &mut Bot, run: &Run, feet: V3, float: P, float_alive: bool, carrying: bool) {
    let mask = run.mask;
    let on_roof = run.ceiling.is_some_and(|c| feet.y >= g.roof_top(feet, c, mask) - 0.1);
    let float_end = if carrying && run.regrabbed {
        FloatEnd::Regrabbed
    } else if carrying {
        FloatEnd::InHand
    } else if float_alive {
        let at = g.pos(float);
        let top_y = g.float_top(float);
        let top = run.ceiling.zip(top_y).map(|(c, top)| top - g.roof_top(at, c, mask));
        FloatEnd::Landed { top, fell: run.spawn.map_or(0.0, |s| s.y - at.y), rise: top_y.unwrap_or(at.y) - run.start_y }
    } else {
        FloatEnd::Lost
    };
    let o = Outcome {
        params: run.params,
        climbed: run.best_y - run.start_y,
        on_roof,
        float: float_end,
        ceiling: run.ceiling.map(|c| c - run.start_y),
        boosts: run.boosts,
        end: run.end.unwrap_or(End::Timeout),
        shared: false,
    };
    bot.release = true;
    match run.kind {
        Kind::Try => {
            bot.done += 1;
            bot.search.add(o);
            append_shared(&to_line(&o, std::process::id()));
            let of = if bot.wanted == u32::MAX { String::new() } else { format!("/{}", bot.wanted) };
            say(&format!("#{}{of} {}", bot.done, o.summary()));
            report(bot, run.total);
            if o.success() {
                say(&format!("    {}", o.params.text()));
                if bot.turbo.is_some() {
                    say("    Reussi en mode rapide : je le rejoue a vitesse normale pour verifier.");
                    bot.run = Some(Run::new(o.params, Kind::Verify));
                } else {
                    win(g, bot, &o);
                }
            }
        },
        Kind::Verify if o.success() => {
            say(&format!("Verifie a vitesse normale : {}", o.summary()));
            win(g, bot, &o);
        },
        Kind::Verify => say(&format!("A vitesse normale ca ne passe pas ({}) : je continue.", o.summary())),
        Kind::Replay => say(&format!("Rejoue : {}", o.summary())),
    }
}

/// A try that works: noted in the shared file, and the end of the automatic mode.
unsafe fn win(g: &G, bot: &mut Bot, o: &Outcome) {
    append_shared(&format!("GAGNE {}", o.params.text()));
    if bot.auto {
        bot.won = true;
        stop(g, bot, "GAGNE ! Le joueur et la bouee sont passes. 'rejouer 1' pour le revoir.");
    }
}

/// Every REPORT_EVERY tries: how many a minute, and how much faster than real time.
fn report(bot: &mut Bot, game_seconds: f32) {
    let (since, game, n) = &mut bot.report;
    (*game, *n) = (*game + game_seconds, *n + 1);
    if *n >= REPORT_EVERY {
        let real = since.elapsed().as_secs_f32().max(0.001);
        say(&format!("  {n} essais en {real:.0} s : {:.0} par minute, jeu {:.1}x plus vite que le temps reel", *n as f32 * 60.0 / real, *game / real));
        bot.report = (Instant::now(), 0.0, 0);
    }
}

unsafe extern "C-unwind" fn pc_update(this: P, mi: usize) {
    let orig: unsafe extern "C-unwind" fn(P, usize) = std::mem::transmute(O_UPDATE.load(Relaxed));
    let ours = GAME.get().filter(|g| !this.is_null() && this == call!(g.player_controller, fn() -> P));
    if let Some(g) = ours {
        // During a try, the bot holds the move stick and the sprint (the input callbacks ran before
        // this Update), and lets go of them once after.
        let mut bot = BOT.lock().unwrap_or_else(|e| e.into_inner());
        let held = match bot.run {
            Some(run) => Some(run.stick()),
            None if bot.release => Some(([0.0; 2], false)),
            None => None,
        };
        if let Some((stick, sprint)) = held {
            wr(this, g.off_move_dir, stick);
            wr(this, g.off_run_hold, sprint);
        }
        if bot.run.is_none() {
            bot.release = false;
        }
    }
    orig(this, mi);
    if let Some(g) = ours {
        let mut bot: MutexGuard<Bot> = BOT.lock().unwrap_or_else(|e| e.into_inner());
        tick(g, &mut bot, this);
    }
}

/// Main menu, automatic mode: Continue the saved game (what its Continue button leads to).
unsafe extern "C-unwind" fn menu_update(this: P, mi: usize) {
    let orig: unsafe extern "C-unwind" fn(P, usize) = std::mem::transmute(O_MENU_UPDATE.load(Relaxed));
    orig(this, mi);
    let (Some(g), false) = (GAME.get(), this.is_null()) else { return };
    let Some(menu) = &g.menu else { return };
    let mut bot = BOT.lock().unwrap_or_else(|e| e.into_inner());
    if !bot.auto || bot.won || bot.continued.is_some_and(|t| t.elapsed() < MENU_RETRY) {
        return;
    }
    bot.continued = Some(Instant::now());
    call!(menu.refresh, fn(P), this);
    let saved: P = il2cpp::rd(this, menu.off_saved);
    if saved.is_null() {
        say("Mode auto : pas de sauvegarde a continuer. Lance une partie, va dans les Poolrooms, puis reviens au menu.");
        return;
    }
    say("Mode auto : je charge ta sauvegarde (Continuer).");
    call!(menu.continue_game, fn(P, P, i32, bool), this, saved, 1, true);
}

unsafe fn open_console() {
    let _ = AllocConsole();
    // The game may have inherited someone else's std handles (pipes from whatever started it), which
    // AllocConsole leaves alone: point them at this console.
    let console = |name: &str| std::fs::OpenOptions::new().read(true).write(true).open(name).ok();
    for (which, name) in [(STD_INPUT_HANDLE, "CONIN$"), (STD_OUTPUT_HANDLE, "CONOUT$"), (STD_ERROR_HANDLE, "CONOUT$")] {
        if let Some(file) = console(name) {
            // Kept open for the life of the process.
            let _ = SetStdHandle(which, HANDLE(file.into_raw_handle()));
        }
    }
    let _ = SetConsoleTitleW(w!("Bot bouee - Inside the Backrooms"));
    // Closing a process's console kills the process: here, the game. Take the close button away.
    let menu = GetSystemMenu(GetConsoleWindow(), false);
    let _ = DeleteMenu(menu, SC_CLOSE, MF_BYCOMMAND);
}

unsafe fn start(hmodule: HINSTANCE) {
    let mut buf = [0u16; 1024];
    let n = GetModuleFileNameW(Some(HMODULE(hmodule.0)), &mut buf) as usize;
    let dll = PathBuf::from(String::from_utf16_lossy(&buf[..n]));
    _ = DIR.set(dll.parent().map_or_else(PathBuf::new, |d| d.to_path_buf()));
    open_console();
    say("Bot bouee (Poolrooms). Ne ferme pas cette fenetre : ca fermerait le jeu.");
    let ready = (0..240).any(|_| {
        let ok = il2cpp::init();
        if !ok {
            std::thread::sleep(Duration::from_millis(500));
        }
        ok
    });
    if !ready {
        say("Jeu pas pret (il2cpp introuvable), abandon.");
        return;
    }
    let thread = (il2cpp::api().thread_attach)((il2cpp::api().domain_get)());
    let resolved = game::resolve();
    (il2cpp::api().thread_detach)(thread);
    let g = match resolved {
        Ok(g) => GAME.get_or_init(|| g),
        Err(e) => {
            say(&format!("Introuvable dans le jeu : {e}"));
            return;
        },
    };
    let mut hooks = vec![(g.pc_update, pc_update as *const () as usize, &O_UPDATE)];
    if let Some(menu) = &g.menu {
        hooks.push((menu.update, menu_update as *const () as usize, &O_MENU_UPDATE));
    }
    let hooked = MH_Initialize().ok().and_then(|_| {
        for (target, detour, orig) in hooks {
            let h = MhHook::new(target.0 as _, detour as *mut c_void)?;
            orig.store(h.trampoline() as usize, Relaxed);
            h.queue_enable()?;
        }
        MH_ApplyQueued().ok()
    });
    if let Err(e) = hooked {
        say(&format!("Hook impossible : {e:?}"));
        return;
    }
    say(HELP);
    let config = read_config();
    {
        let mut bot = BOT.lock().unwrap_or_else(|e| e.into_inner());
        (bot.auto, bot.speed, bot.fast) = (config.auto, config.speed, config.fast);
    }
    if config.auto {
        say(&match (shared_start(), g.menu.is_some()) {
            (Some(s), true) => format!("Mode auto : depart connu (bouee '{}'). Depuis le menu je charge ta sauvegarde, puis je cherche.", s.float_name),
            (Some(_), false) => "Mode auto : depart connu. Cette version n'a pas de sauvegarde a continuer : va dans les Poolrooms.".into(),
            (None, _) => "Mode auto : premier lancement. Dans les Poolrooms, mets-toi au depart avec la licorne en main et tape 'depart'.".into(),
        });
    }
    // This thread now reads the console.
    let mut line = String::new();
    loop {
        line.clear();
        if std::io::stdin().read_line(&mut line).unwrap_or(0) == 0 {
            std::thread::sleep(Duration::from_millis(200));
            continue;
        }
        match parse(line.trim()) {
            Ok(cmd) => COMMANDS.lock().unwrap_or_else(|e| e.into_inner()).push(cmd),
            Err(e) => say(&e),
        }
    }
}

#[no_mangle]
pub unsafe extern "system" fn DllMain(hmodule: HINSTANCE, reason: u32, _: *mut c_void) -> i32 {
    if reason == 1 {
        // DLL_PROCESS_ATTACH: nothing heavy under the loader lock.
        let raw = hmodule.0 as usize;
        std::thread::spawn(move || start(HINSTANCE(raw as _)));
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands() {
        assert_eq!(parse("lancer"), Ok(Cmd::Lancer(200)));
        assert_eq!(parse("LANCER 50"), Ok(Cmd::Lancer(50)));
        assert_eq!(parse("vitesse 2,5"), Ok(Cmd::Vitesse(2.5)));
        assert_eq!(parse("vitesse 9"), Ok(Cmd::Vitesse(4.0)));
        assert_eq!(parse("rejouer 3"), Ok(Cmd::Rejouer(3)));
        assert_eq!(parse("départ"), Ok(Cmd::Depart));
        assert_eq!(parse("auto"), Ok(Cmd::Auto(true)));
        assert_eq!(parse("auto non"), Ok(Cmd::Auto(false)));
        assert_eq!(parse("rapide non"), Ok(Cmd::Rapide(false)));
        assert_eq!(parse("enregistre"), Ok(Cmd::Enregistre));
        assert!(parse("auto peut-etre").is_err());
        assert!(parse("vitesse").is_err() && parse("sauter").is_err());
    }

    #[test]
    fn stick_by_phase() {
        // Looking 90 to the left of where it runs: the stick points right.
        let p = Params::from_array([0.0, 2.0, -90.0, 0.0, 0.6, 1.0, 0.5, 0.1, 0.1, 0.2, -0.5, 3.0]);
        let near = |(a, run): ([f32; 2], bool), (b, sprint): ([f32; 2], bool)| {
            (a[0] - b[0]).abs() < 1e-5 && (a[1] - b[1]).abs() < 1e-5 && run == sprint
        };
        let mut run = Run::new(p, Kind::Try);
        assert!(near(run.stick(), ([0.0; 2], false)));
        run.go(Phase::RunUp);
        assert!(near(run.stick(), ([1.0, 0.0], true)), "{:?}", run.stick());
        run.go(Phase::Grab);
        assert!(near(run.stick(), ([0.6, 0.0], true)));
        run.go(Phase::Measure);
        assert!(near(run.stick(), ([0.0; 2], false)));
        let walking = Run { phase: Phase::RunUp, ..Run::new(Params { sprint: 0.2, ..p }, Kind::Try) };
        assert!(!walking.stick().1);
    }
}
