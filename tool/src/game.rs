// Game side: resolves the game's methods/fields by name, hooks PlayerController.Update and
// PlayerCamera.Update to run our logic on Unity's main thread, and exchanges data with the overlay
// through STATE.
use std::collections::{BTreeMap, HashMap};
use std::f32::consts::{PI, TAU};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use hudhook::mh::{MhHook, MH_ApplyQueued};
use hudhook::windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, MapVirtualKeyW, MAPVK_VSC_TO_VK, VK_LCONTROL, VK_LSHIFT, VK_SPACE,
};

use crate::call;
use crate::il2cpp::{self, alive, class, field, field_of, method, method_sig, rd, wr, M, P};

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct V3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl V3 {
    fn add_scaled(self, o: V3, k: f32) -> V3 {
        V3 { x: self.x + o.x * k, y: self.y + o.y * k, z: self.z + o.z * k }
    }

    fn sub(self, o: V3) -> V3 {
        self.add_scaled(o, -1.0)
    }

    fn scale(self, k: f32) -> V3 {
        V3::default().add_scaled(self, k)
    }

    fn dot(self, o: V3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    fn cross(self, o: V3) -> V3 {
        V3 { x: self.y * o.z - self.z * o.y, y: self.z * o.x - self.x * o.z, z: self.x * o.y - self.y * o.x }
    }

    pub fn dist(self, o: V3) -> f32 {
        ((self.x - o.x).powi(2) + (self.y - o.y).powi(2) + (self.z - o.z).powi(2)).sqrt()
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Quat([f32; 4]);

impl Quat {
    /// Turn of `deg` degrees around the world's up axis (positive: from +Z toward +X, like Unity).
    fn yaw(deg: f32) -> Quat {
        let h = deg.to_radians() / 2.0;
        Quat([0.0, h.sin(), 0.0, h.cos()])
    }

    /// `self` applied after `o`.
    fn mul(self, o: Quat) -> Quat {
        let ([ax, ay, az, aw], [bx, by, bz, bw]) = (self.0, o.0);
        Quat([
            aw * bx + ax * bw + ay * bz - az * by,
            aw * by - ax * bz + ay * bw + az * bx,
            aw * bz + ax * by - ay * bx + az * bw,
            aw * bw - ax * bx - ay * by - az * bz,
        ])
    }

    /// Normalized lerp along the shortest way: plenty for two close aims.
    fn nlerp(self, o: Quat, t: f32) -> Quat {
        let dot: f32 = (0..4).map(|k| self.0[k] * o.0[k]).sum();
        let sign = if dot < 0.0 { -1.0 } else { 1.0 };
        let q = [0, 1, 2, 3].map(|k| self.0[k] + (o.0[k] * sign - self.0[k]) * t);
        let len = q.iter().map(|v| v * v).sum::<f32>().sqrt();
        Quat(q.map(|v| v / len))
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Bounds {
    center: V3,
    extents: V3,
}

/// Unity Matrix4x4: column-major, element (row r, col c) at [c * 4 + r].
#[repr(C)]
#[derive(Clone, Copy)]
struct Mat4([f32; 16]);

impl Mat4 {
    fn mul(&self, b: &Mat4) -> Mat4 {
        let mut m = [0.0; 16];
        for c in 0..4 {
            for r in 0..4 {
                m[c * 4 + r] = (0..4).map(|k| self.0[k * 4 + r] * b.0[c * 4 + k]).sum();
            }
        }
        Mat4(m)
    }

    fn point(&self, p: V3) -> [f32; 4] {
        let m = &self.0;
        [0, 1, 2, 3].map(|r| m[r] * p.x + m[4 + r] * p.y + m[8 + r] * p.z + m[12 + r])
    }

    /// Point through an affine matrix (local to world and back).
    fn apply(&self, p: V3) -> V3 {
        let [x, y, z, _] = self.point(p);
        V3 { x, y, z }
    }

    /// World point to normalized device coordinates, None if behind the camera.
    fn project(&self, p: V3) -> Option<[f32; 2]> {
        let [x, y, _, w] = self.point(p);
        (w > 0.05).then(|| [x / w, y / w])
    }
}

#[derive(Clone)]
pub struct Saved {
    pub pos: V3,
    rot: Quat,
    stats: [f32; 4],
    pub items: Vec<(String, i32)>,
    /// Poolrooms float carried when saved (0 if none): handed back on load.
    pub float: usize,
    /// PlayerController.lookDirection: y is the camera's vertical angle (x the horizontal one).
    look: V3,
    /// Moth jelly energy boost (on, seconds since eaten).
    pub boost: (bool, f32),
}

/// Where the carried float would end up if dropped now, in world space.
#[derive(Clone, Copy)]
pub struct DropInfo {
    pos: V3,
    radius: f32,
    land: Option<V3>,
    inside: bool,
}

impl DropInfo {
    pub fn label(&self) -> String {
        match (self.inside, self.land.map(|l| self.pos.y - l.y)) {
            (true, _) => "dans le decor : sera ejectee".into(),
            (_, None) => "rien dessous : tombe dans le vide".into(),
            (_, Some(fall)) if fall > 1.0 => format!("retombe de {fall:.1} m"),
            (_, Some(fall)) => format!("reste en haut ({fall:.1} m)"),
        }
    }

    /// The green case: it stays at this height (on the roof).
    fn stays_up(&self) -> bool {
        !self.inside && self.land.is_some_and(|l| self.pos.y - l.y <= 1.0)
    }

    pub fn color(&self) -> [f32; 4] {
        match (self.inside, self.land.map(|l| self.pos.y - l.y)) {
            (true, _) => [1.0, 0.6, 0.1, 1.0],
            (_, Some(fall)) if fall <= 1.0 => [0.2, 1.0, 0.3, 1.0],
            (_, Some(fall)) if fall <= 3.0 => [1.0, 0.6, 0.1, 1.0],
            _ => [1.0, 0.2, 0.2, 1.0],
        }
    }
}

/// Looping test: savestates reloaded, the float dropped after each, until stopped. It learns which
/// (spot, head turn) works: every spot and every turn giving the green preview gets tried, then the
/// ones that work more often.
/// Frames between the reload and the drop: the float settles in the hands meanwhile.
const SETTLE_FRAMES: u32 = 30;
/// Game seconds after the drop before judging: pushed through the roof and float grabbed by then.
const WATCH_SECS: f32 = 3.0;
/// Head turns tried at each spot: TURNS of them, SEEK_STEP degrees apart, centred on the slot's aim.
pub const TURNS: usize = 41;
const SEEK_STEP: f32 = 3.0;
/// Frames per turn while sweeping them for the green preview (it follows the body a frame late).
const SEEK_HOLD: u32 = 3;
/// Before dropping, the preview must be green this many frames in a row, within HOLD_FRAMES.
const GREEN_FRAMES: u32 = 3;
const HOLD_FRAMES: u32 = 20;
/// Rising at least this much (m) after the drop means the float pushed us through the roof.
const ROOF_RISE: f32 = 1.0;

/// Gap between two tested spots on the line between the two slots (m), and how many spots at most.
const SPOT_STEP: f32 = 0.05;
const MAX_SPOTS: usize = 41;

pub struct Repeat {
    /// The slot the test starts from and, to sweep the spots in between, the one at the other end.
    pub from: usize,
    pub to: Option<usize>,
    phase: Phase,
    /// The spot being tried and its head turn (None: its green turns aren't known yet, sweep first).
    spot: usize,
    turn: Option<usize>,
    /// The spot's body rotation and lookDirection, before any head turn.
    aim: (Quat, V3),
    /// For each spot, which turns give the green preview (None until swept).
    greens: Vec<Option<Vec<bool>>>,
    scan: Vec<bool>,
    /// Where the bot aimed when it grabbed the float back, and the preview's side at the drop.
    grab: Option<V3>,
    side: Option<Side>,
}

impl Repeat {
    pub fn new(from: usize, to: Option<usize>) -> Self {
        Repeat {
            from,
            to,
            phase: Phase::Load,
            spot: 0,
            turn: None,
            aim: Default::default(),
            greens: Vec::new(),
            scan: Vec::new(),
            grab: None,
            side: None,
        }
    }
}

/// How many spots get tried: one every SPOT_STEP from `a` to `b`, or just `a`.
pub fn spot_count(a: &Saved, b: Option<&Saved>) -> usize {
    b.map_or(1, |b| ((a.pos.dist(b.pos) / SPOT_STEP).round() as usize + 1).min(MAX_SPOTS))
}

/// Spot `i` of `n`: position and aim blended between the two slots, the rest from `a`.
pub fn spot_at(a: &Saved, b: Option<&Saved>, i: usize, n: usize) -> Saved {
    let Some(b) = b.filter(|_| n > 1) else { return a.clone() };
    let t = i as f32 / (n - 1) as f32;
    let lerp = |p: V3, q: V3| p.add_scaled(q.sub(p), t);
    Saved { pos: lerp(a.pos, b.pos), rot: a.rot.nlerp(b.rot, t), look: lerp(a.look, b.look), ..a.clone() }
}

#[derive(Clone, Copy)]
enum Phase {
    Load,
    Settle(u32),
    /// Turning the head through every turn to see which ones give the green preview.
    Scan { k: usize, frame: u32 },
    /// Holding the chosen turn until the preview is steadily green, then dropping.
    Hold { frame: u32, green: u32 },
    Watch { t: f32, start_y: f32 },
}

/// Head turn (degrees) of turn index `k`: -60 for 0, 0 in the middle, +60 for the last.
pub fn turn_deg(k: usize) -> f32 {
    (k as f32 - (TURNS / 2) as f32) * SEEK_STEP
}

/// Direction a rotation faces, in degrees (0 = +Z, positive toward +X, i.e. turning right).
fn yaw_of(q: Quat) -> f32 {
    let [x, y, z, w] = q.0;
    (2.0 * (x * z + w * y)).atan2(1.0 - 2.0 * (x * x + y * y)).to_degrees()
}

/// Where the drop preview sits around the player.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Side {
    Front,
    Right,
    Back,
    Left,
}

impl Side {
    pub const ALL: [Side; 4] = [Side::Front, Side::Right, Side::Back, Side::Left];

    pub fn label(self) -> &'static str {
        match self {
            Side::Front => "devant",
            Side::Right => "a droite",
            Side::Back => "derriere",
            Side::Left => "a gauche",
        }
    }
}

/// Side of `drop` for a player at `player` facing `yaw` degrees (see yaw_of).
fn drop_side(player: V3, yaw: f32, drop: V3) -> Side {
    let d = drop.sub(player);
    let a = (d.x.atan2(d.z).to_degrees() - yaw + 540.0).rem_euclid(360.0) - 180.0;
    match a.abs() {
        x if x <= 45.0 => Side::Front,
        x if x >= 135.0 => Side::Back,
        _ if a > 0.0 => Side::Right,
        _ => Side::Left,
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Outcome {
    Success,
    HornHidden,
    NotThrough,
    NeverGreen,
}

impl Outcome {
    pub const ALL: [Outcome; 4] = [Outcome::Success, Outcome::HornHidden, Outcome::NotThrough, Outcome::NeverGreen];

    pub fn label(self) -> &'static str {
        match self {
            Outcome::Success => "reussi : sur le toit avec la bouee",
            Outcome::HornHidden => "sur le toit, bouee pas attrapable",
            Outcome::NotThrough => "pas passe a travers le toit",
            Outcome::NeverGreen => "pas de cercle vert (pas lachee)",
        }
    }

    pub fn color(self) -> [f32; 4] {
        match self {
            Outcome::Success => [0.2, 1.0, 0.3, 1.0],
            Outcome::HornHidden => [1.0, 0.6, 0.1, 1.0],
            Outcome::NotThrough => [1.0, 0.2, 0.2, 1.0],
            Outcome::NeverGreen => [0.6, 0.6, 0.6, 1.0],
        }
    }
}

pub struct Trial {
    pub spot: usize,
    /// Head turn index (None: no turn of this spot gave the green preview).
    pub turn: Option<usize>,
    pub outcome: Outcome,
    /// Where the float ended up.
    pub float_pos: V3,
    /// Where the bot aimed to grab it back, when it did.
    pub grab: Option<V3>,
    /// Where the green preview was around the player at the drop.
    pub side: Option<Side>,
}

/// `rise`: how much the player went up since the drop; `holding`: the float is back in the hands.
fn classify(rise: f32, holding: bool) -> Outcome {
    match (rise > ROOF_RISE, holding) {
        (true, true) => Outcome::Success,
        (true, false) => Outcome::HornHidden,
        _ => Outcome::NotThrough,
    }
}

/// Largest distance between two points: for final float positions, 0 means the game did exactly the same every time.
pub fn spread(pts: &[V3]) -> f32 {
    pts.iter().flat_map(|a| pts.iter().map(move |b| a.dist(*b))).fold(0.0, f32::max)
}

/// (tries, successes) of each spot, whatever the turn.
pub fn spot_stats(trials: &[Trial], spots: usize) -> Vec<(usize, usize)> {
    let mut out = vec![(0, 0); spots];
    for t in trials.iter().filter(|t| t.spot < spots) {
        out[t.spot].0 += 1;
        out[t.spot].1 += (t.outcome == Outcome::Success) as usize;
    }
    out
}

/// (tries, successes) of each (spot, turn) cell: cell = spot * TURNS + turn.
fn cell_stats(trials: &[Trial], spots: usize) -> Vec<(usize, usize)> {
    let mut out = vec![(0, 0); spots * TURNS];
    for t in trials.iter().filter(|t| t.spot < spots) {
        if let Some(k) = t.turn {
            let c = &mut out[t.spot * TURNS + k];
            c.0 += 1;
            c.1 += (t.outcome == Outcome::Success) as usize;
        }
    }
    out
}

/// A cell with its neighbours one row and one column away (`cols` per row): for a line up, what
/// you get standing 5 cm off or aiming 3 degrees off.
fn zone_stats(stats: &[(usize, usize)], cols: usize, cell: usize) -> (usize, usize) {
    let (row, col, rows) = (cell / cols, cell % cols, stats.len() / cols);
    let mut out = (0, 0);
    for r in row.saturating_sub(1)..(row + 2).min(rows) {
        for c in col.saturating_sub(1)..(col + 2).min(cols) {
            out.0 += stats[r * cols + c].0;
            out.1 += stats[r * cols + c].1;
        }
    }
    out
}

/// The tried cell that works most reliably even a bit off: best (successes + 1) / (tries + 2) over
/// its zone, so 1/1 doesn't beat 9/10 and a lucky cell among failures doesn't beat a wide working area.
fn best_cell(stats: &[(usize, usize)], cols: usize) -> Option<usize> {
    let score = |c: usize| {
        let (tries, ok) = zone_stats(stats, cols, c);
        (ok as f32 + 1.0) / (tries as f32 + 2.0)
    };
    (0..stats.len()).filter(|&c| stats[c].0 > 0).max_by(|&i, &j| score(i).total_cmp(&score(j)))
}

/// Cells whose turn gave the green preview at their spot (only spot `only`'s if given).
fn green_cells(greens: &[Option<Vec<bool>>], only: Option<usize>) -> Vec<usize> {
    let mut out = Vec::new();
    for (i, g) in greens.iter().enumerate().filter(|(i, _)| only.is_none_or(|o| o == *i)) {
        out.extend(g.iter().flatten().enumerate().filter(|(_, &ok)| ok).map(|(k, _)| i * TURNS + k));
    }
    out
}

/// Cell to try next among `candidates`: untried ones first (closest to the slot's own aim), then
/// UCB1: the success rate plus a bonus for few tries, so the test spends its time on what works
/// while still rechecking the rest.
fn next_cell(stats: &[(usize, usize)], candidates: &[usize]) -> Option<usize> {
    let untried = candidates.iter().copied().filter(|&c| stats[c].0 == 0);
    if let Some(c) = untried.min_by_key(|&c| (c % TURNS).abs_diff(TURNS / 2)) {
        return Some(c);
    }
    let total = candidates.iter().map(|&c| stats[c].0).sum::<usize>() as f32;
    let ucb = |c: usize| {
        let (tries, ok) = stats[c];
        ok as f32 / tries as f32 + (2.0 * total.ln() / tries as f32).sqrt()
    };
    candidates.iter().copied().max_by(|&i, &j| ucb(i).total_cmp(&ucb(j)))
}

/// The best line up the test found: where to stand, which way to face, where to aim for the horn.
pub struct Lineup {
    /// The spot with the head turned, ready to become a slot.
    pub saved: Saved,
    /// Direction to face (degrees, see yaw_of) and head turn from the first slot's aim.
    pub yaw: f32,
    pub turn: f32,
    /// Distance from the first slot (m).
    pub away: f32,
    /// (tries, successes) of the line up itself, and with 5 cm / 3 degrees of error.
    pub own: (usize, usize),
    pub zone: (usize, usize),
    pub grab: Option<V3>,
    pub side: Option<Side>,
    /// Where the float ended up each time.
    pub ends: Vec<V3>,
}

pub fn lineup(s: &State) -> Option<Lineup> {
    let (from, to) = s.run_slots;
    let a = s.slots.get(from)?;
    let b = to.and_then(|i| s.slots.get(i));
    let spots = spot_count(a, b);
    let stats = cell_stats(&s.trials, spots);
    let best = best_cell(&stats, TURNS)?;
    let (spot, k) = (best / TURNS, best % TURNS);
    let mut saved = spot_at(a, b, spot, spots);
    saved.rot = Quat::yaw(turn_deg(k)).mul(saved.rot);
    let at_best = || s.trials.iter().filter(|t| t.spot == spot && t.turn == Some(k));
    Some(Lineup {
        yaw: yaw_of(saved.rot),
        turn: turn_deg(k),
        away: saved.pos.dist(a.pos),
        own: stats[best],
        zone: zone_stats(&stats, TURNS, best),
        grab: at_best().filter_map(|t| t.grab).last(),
        side: at_best().find_map(|t| t.side),
        ends: at_best().map(|t| t.float_pos).collect(),
        saved,
    })
}

pub struct Point {
    pub label: String,
    pub pos: V3,
}

pub struct EntityInfo {
    pub id: usize,
    pub name: String,
    pub pos: V3,
    pub active: bool,
}

/// A line in normalized device coordinates (-1..1, y up) for the overlay to draw.
pub struct Line {
    pub a: [f32; 2],
    pub b: [f32; 2],
    pub color: [f32; 4],
}

/// A filled convex polygon in normalized device coordinates.
pub struct Poly {
    pub pts: Vec<[f32; 2]>,
    pub color: [f32; 4],
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Solid,
    Invisible,
    Trigger,
    Special,
    /// On a layer the player never touches: not drawn.
    Ignored,
}

/// Collision shape in the collider's local space, drawn through its transform every frame.
#[derive(Default)]
struct Shape {
    edges: Vec<(V3, V3)>,
    faces: Vec<[V3; 3]>,
    /// Bounding box of a mesh that can't be read, instead of its real triangles.
    approx: bool,
}

struct Info {
    kind: Kind,
    label: String,
    shape: Shape,
}

#[derive(Clone)]
struct Col {
    obj: usize,
    info: Arc<Info>,
    bounds: Bounds,
}

pub enum Action {
    Save,
    Load(usize),
    /// Load a savestate that isn't in the slot list (a spot of the looping test).
    LoadAt(Saved),
    Give(String),
    Goto(V3),
    ToggleEntity(usize),
    ScanPoints,
    /// Open every level for good, the way reaching them does (writes the game's Steam stats).
    UnlockLevelsForever,
}

// Read by hooks too, so kept outside the mutex.
pub static GOD: AtomicBool = AtomicBool::new(false);
pub static NOCLIP: AtomicBool = AtomicBool::new(false);
pub static INF_ITEMS: AtomicBool = AtomicBool::new(false);
pub static NO_FALL_TP: AtomicBool = AtomicBool::new(false);
/// Every level counts as unlocked in the level select (v0.7.0 keeps unlocks in Steam stats).
pub static UNLOCK_LEVELS: AtomicBool = AtomicBool::new(false);

#[derive(Default)]
pub struct State {
    pub menu_open: bool,
    pub inf_stamina: bool,
    pub inf_oxygen: bool,
    pub no_anxiety: bool,
    pub freeze_entities: bool,
    pub fly_speed: f32,
    pub time_scale: f32,
    pub speed_mult: f32,
    pub jump_mult: f32,
    pub fx_off: bool,
    pub level_post_off: bool,
    pub brightness: f32,
    pub restore_inventory: bool,
    pub drop_preview: bool,
    pub drop: Option<DropInfo>,
    pub show_float: bool,
    pub show_trail: bool,
    /// Top of the last dropped float above the ground under the player (m): on the roof, > 0 sticks out.
    pub float_top: Option<f32>,
    pub repeat: Option<Repeat>,
    /// Skip head turns putting the green preview on your right (the float tends to fall there).
    pub skip_right: bool,
    /// 1-based slot to sweep to from the selected one, 0 for none.
    pub sweep_to: i32,
    /// (from, to) of the last test run, for its results.
    pub run_slots: (usize, Option<usize>),
    pub trials: Vec<Trial>,
    pub repeat_error: Option<&'static str>,
    /// The permanent level unlock waits for a second click; its result.
    pub unlock_confirm: bool,
    pub unlock_msg: Option<String>,
    pub show_colliders: bool,
    pub show_visible_walls: bool,
    pub show_triggers: bool,
    pub collider_radius: f32,
    pub actions: Vec<Action>,
    pub slots: Vec<Saved>,
    pub selected: usize,
    // Filled by the game thread for the overlay.
    pub last_tick: Option<Instant>,
    pub pos: V3,
    /// Direction the player faces (degrees, see yaw_of).
    pub yaw: f32,
    pub stats: [f32; 4], // health, stamina, oxygen, anxiety
    pub game_time: f32,
    pub fall_time: f32,
    pub ground_below: bool,
    pub items: Vec<String>,
    pub points: Vec<Point>,
    pub entities: Vec<EntityInfo>,
    pub lines: Vec<Line>,
    pub polys: Vec<Poly>,
    pub labels: Vec<([f32; 2], String, [f32; 4])>,
    pub collider_count: usize,
    pub approx_count: usize,
    // Game-thread bookkeeping.
    stats_obj: usize,
    max: [f32; 4],
    fly_pos: V3,
    applied_god: bool,
    applied_noclip: bool,
    applied_freeze: bool,
    applied_time_scale: f32,
    applied_mults: (f32, f32),
    vars_orig: Option<(usize, Vec<f32>)>,
    cursor_saved: Option<(i32, bool)>,
    fx_saved: BTreeMap<usize, bool>,
    vhs_saved: BTreeMap<usize, f32>,
    post_saved: BTreeMap<usize, f32>,
    exposure: Option<(usize, f32, f32, bool)>, // param, base value, written value, original override
    colliders: Vec<Col>,
    // Keyed by (managed, native) pointer: either alone can be reused by another collider later.
    infos: BTreeMap<(usize, usize), Arc<Info>>,
    last_scan: Option<Instant>,
    camera: usize,
    camera_hook_drew: bool,
    floats: Vec<usize>,
    last_float_scan: Option<Instant>,
    /// (float, collider key, shape) of the floats around, refreshed with the float scan.
    float_cols: Vec<(usize, (usize, usize), Arc<Info>)>,
    float_shapes: BTreeMap<(usize, usize), Arc<Info>>,
    carried: usize,
    /// The last float we dropped, its path since then and for how long it has been recorded.
    trail_float: usize,
    trail: Vec<V3>,
    trail_age: f32,
    float_top_pos: Option<V3>,
}

pub static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| {
    Mutex::new(State {
        fly_speed: 8.0,
        time_scale: 1.0,
        applied_time_scale: 1.0,
        speed_mult: 1.0,
        jump_mult: 1.0,
        applied_mults: (1.0, 1.0),
        restore_inventory: true,
        collider_radius: 25.0,
        drop_preview: true,
        show_float: true,
        show_trail: true,
        skip_right: true,
        ..Default::default()
    })
});

pub fn lock() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

// Trigger components worth flagging in the collider view: (namespace, class, label).
const SPECIAL_TRIGGERS: [(&str, &str, &str); 11] = [
    ("", "ITBTeleportTrigger", "TELEPORT"),
    ("", "PlayerTeleporter", "TELEPORT"),
    ("", "LightSwitchZoneTeleporter", "TELEPORT"),
    ("", "RespawnTrigger", "RESPAWN"),
    ("", "BackroomsLevelSwapTrigger", "CHANGEMENT NIVEAU"),
    ("", "BackroomsExitZone", "SORTIE"),
    ("", "UnrespawneableZone", "ZONE SANS RESPAWN"),
    ("ITB.Scenario", "ITBPlayerDamager", "DEGATS"),
    ("", "LevelUnlockTrigger", "DEBLOCAGE NIVEAU"),
    ("", "AutoSaveTrigger", "AUTOSAVE"),
    ("", "SubsceneTrigger", "CHARGEMENT ZONE"),
];

const ELEVATORS: [&str; 4] = ["MainElevator", "Elevator", "HotelElevator", "LiftElevator"];

struct G {
    player_controller: M,
    player_stats: M,
    player_inventory: M,
    player_camera: M,
    player_vars: M,
    game_time: M,
    teleport: M,
    teleport_hold: bool, // v0.8.0 added Teleport(.., holdForGround)
    comp_transform: M,
    comp_gameobject: M,
    get_component: M,
    obj_name: M,
    find_objects: M,
    tr_get_pos: M,
    tr_set_pos: M,
    tr_get_rot: M,
    tr_set_rot: M,
    tr_forward: M,
    tr_right: M,
    tr_matrix: M,
    go_set_active: M,
    go_active: M,
    time_set_scale: M,
    delta_time: M,
    sync_transforms: M,
    cursor_lock_get: M,
    cursor_lock_set: M,
    cursor_visible_get: M,
    cursor_visible_set: M,
    cam_view: M,
    cam_proj: M,
    col_bounds: M,
    col_enabled: M,
    col_trigger: M,
    box_center: M,
    box_size: M,
    sphere_center: M,
    sphere_radius: M,
    capsule_center: M,
    capsule_radius: M,
    capsule_height: M,
    capsule_direction: M,
    mc_mesh: M,
    mc_convex: M,
    mesh_readable: M,
    mesh_bounds: M,
    mesh_vertices: M,
    mesh_triangles: M,
    col_raycast: M,
    tr_inverse: M,
    get_component_in_children: M,
    renderer_enabled: M,
    go_layer: M,
    ignore_layers: M,
    /// Stripped from the v0.7.0 build: then no per-collider-pair exceptions are considered.
    ignore_pair: Option<M>,
    item_names: M,
    add_item: M,
    remove_item: M,
    item_count: M,
    carry_drop_pos: M,
    carry_pickup: M,
    carry_drop: M,
    align_pitch: M,
    player_interact: M,
    net_id: M,
    tr_scale: M,
    sphere_cast: M,
    check_sphere: M,
    off_carrier: usize,
    off_floater_radius: usize,
    off_float_colliders: usize,
    t_carry: P,
    t_identity: P,
    entities_singleton: M,
    gm_singleton: M,
    bm_singleton: M,
    bm_level_info: M,
    off_camera: usize,
    off_character: usize,
    off_gravity: usize,
    off_grounded: usize,
    off_falling_time: usize,
    off_falling_inf: usize,
    off_look: usize,
    off_boost: usize,
    off_boost_timer: usize,
    off_stats: [usize; 4],
    off_invuln: usize,
    off_spawned: usize,
    off_vars: Vec<usize>, // speeds then jumpForce (last)
    off_fx: Vec<usize>,
    off_color_adj: usize,
    off_post_exposure: usize,
    off_active: usize,
    off_override: usize,
    off_weight: usize,
    off_gm_vhs: usize,
    off_level_post: usize,
    off_reset_point: usize,
    off_spawnpoints: usize,
    off_tp_points: usize,
    off_exit_id: usize,
    off_swap_level: usize,
    off_tp_dest: usize,
    t_collider: P,
    t_renderer: P,
    k_box: P,
    k_sphere: P,
    k_capsule: P,
    k_mesh: P,
    k_character: P,
    // LevelBoundaries only exists from v0.8.0 on.
    boundaries: Option<Boundaries>,
    t_exit: P,
    t_swap: P,
    t_tp: P,
    t_elevators: Vec<P>,
    specials: Vec<(P, &'static str)>,
    level_names: Vec<(i32, String)>,
    // Hook targets.
    pc_update: M,
    cam_update: M,
    begin_camera: M,
    take_damage: M,
    kill_player: M,
    monster_kill: M,
    infinite_falling: M,
    wear_item: M,
    level_unlocked: M,
    /// v0.7.0: levels open from Steam stats (REACH_POOLROOMS...). Absent elsewhere.
    level_unlock: Option<LevelUnlock>,
}

/// What reaching a level does in v0.7.0: its unlock stat set to 1, then the level opened.
struct LevelUnlock {
    get_info: M,
    off_stat: usize,
    instance: M,
    set_stat: M,
    mark: M,
}

struct Boundaries {
    t: P,
    level: M,
    off_start: usize,
    off_end: usize,
}

// Only holds code/metadata pointers and type objects that il2cpp never frees.
unsafe impl Send for G {}
unsafe impl Sync for G {}

static G: OnceLock<G> = OnceLock::new();

unsafe fn resolve() -> Result<G, String> {
    use il2cpp::type_object as ty;
    let (ac, core, phys) = ("Assembly-CSharp.dll", "UnityEngine.CoreModule.dll", "UnityEngine.PhysicsModule.dll");
    let (rp, hd) = ("Unity.RenderPipelines.Core.Runtime.dll", "Unity.RenderPipelines.HighDefinition.Runtime.dll");
    let k = |name: &str| class(ac, "", name);
    let (player, bpc, pc, stats, inv) =
        (k("Player")?, k("BasePlayerController")?, k("PlayerController")?, k("PlayerStats")?, k("NetworkInventory")?);
    let (items, ents, vars, cam, grounded) =
        (k("ItemList")?, k("EntitiesManager")?, k("PlayerVariablesInfo")?, k("PlayerCamera")?, k("GroundedChecker")?);
    let (gm, bm, level_params) = (k("GameManager")?, k("BackroomsManager")?, k("BackroomLevelParameters")?);
    let (exit, swap, tp) = (k("BackroomsExitZone")?, k("BackroomsLevelSwapTrigger")?, k("ITBTeleportTrigger")?);
    let carry = class(ac, "ITB.SYSTEMS.POOLROOMS", "PoolroomsFloaterCarry")?;
    let identity = class("Mirror.dll", "Mirror", "NetworkIdentity")?;
    let ue = |name: &str| class(core, "UnityEngine", name);
    let (object, comp, tr, go, time, cursor, camera, renderer) = (
        ue("Object")?, ue("Component")?, ue("Transform")?, ue("GameObject")?, ue("Time")?, ue("Cursor")?, ue("Camera")?,
        ue("Renderer")?,
    );
    let mesh = ue("Mesh")?;
    let render_pipeline = class(core, "UnityEngine.Rendering", "RenderPipeline")?;
    let ph = |name: &str| class(phys, "UnityEngine", name);
    let (physics, collider, boxc, character) = (ph("Physics")?, ph("Collider")?, ph("BoxCollider")?, ph("CharacterController")?);
    let (sphere, capsule, meshc) = (ph("SphereCollider")?, ph("CapsuleCollider")?, ph("MeshCollider")?);
    let (vol_comp, volume, vol_param) = (
        class(rp, "UnityEngine.Rendering", "VolumeComponent")?,
        class(rp, "UnityEngine.Rendering", "Volume")?,
        class(rp, "UnityEngine.Rendering", "VolumeParameter")?,
    );
    let color_adj = class(hd, "UnityEngine.Rendering.HighDefinition", "ColorAdjustments")?;
    let f = field;
    Ok(G {
        player_controller: method(player, "get_Controller", 0)?,
        player_stats: method(player, "get_Stats", 0)?,
        player_inventory: method(player, "get_Inventory", 0)?,
        player_camera: method(player, "get_Camera", 0)?,
        player_vars: method(player, "get_vars", 0)?,
        game_time: method(player, "GetCurrentGameTime", 0)?,
        teleport: method(bpc, "Teleport", 3).or_else(|_| method(bpc, "Teleport", 2))?,
        teleport_hold: method(bpc, "Teleport", 3).is_ok(),
        comp_transform: method(comp, "get_transform", 0)?,
        comp_gameobject: method(comp, "get_gameObject", 0)?,
        get_component: method_sig(comp, "GetComponent", &["System.Type"])?,
        obj_name: method(object, "get_name", 0)?,
        // v0.7.0 only has the older FindObjectsOfType(Type, includeInactive): called with 0 like the
        // other (sort mode None), it is the same active-objects-only search.
        find_objects: method_sig(object, "FindObjectsByType", &["System.Type", "UnityEngine.FindObjectsSortMode"])
            .or_else(|_| method_sig(object, "FindObjectsOfType", &["System.Type", "System.Boolean"]))?,
        tr_get_pos: method(tr, "get_position", 0)?,
        tr_set_pos: method(tr, "set_position", 1)?,
        tr_get_rot: method(tr, "get_rotation", 0)?,
        tr_set_rot: method(tr, "set_rotation", 1)?,
        tr_forward: method(tr, "get_forward", 0)?,
        tr_right: method(tr, "get_right", 0)?,
        tr_matrix: method(tr, "get_localToWorldMatrix", 0)?,
        go_set_active: method(go, "SetActive", 1)?,
        go_active: method(go, "get_activeSelf", 0)?,
        time_set_scale: method(time, "set_timeScale", 1)?,
        delta_time: method(time, "get_deltaTime", 0)?,
        sync_transforms: method(physics, "SyncTransforms", 0)?,
        cursor_lock_get: method(cursor, "get_lockState", 0)?,
        cursor_lock_set: method(cursor, "set_lockState", 1)?,
        cursor_visible_get: method(cursor, "get_visible", 0)?,
        cursor_visible_set: method(cursor, "set_visible", 1)?,
        cam_view: method(camera, "get_worldToCameraMatrix", 0)?,
        cam_proj: method(camera, "get_projectionMatrix", 0)?,
        col_bounds: method(collider, "get_bounds", 0)?,
        col_enabled: method(collider, "get_enabled", 0)?,
        col_trigger: method(collider, "get_isTrigger", 0)?,
        box_center: method(boxc, "get_center", 0)?,
        box_size: method(boxc, "get_size", 0)?,
        sphere_center: method(sphere, "get_center", 0)?,
        sphere_radius: method(sphere, "get_radius", 0)?,
        capsule_center: method(capsule, "get_center", 0)?,
        capsule_radius: method(capsule, "get_radius", 0)?,
        capsule_height: method(capsule, "get_height", 0)?,
        capsule_direction: method(capsule, "get_direction", 0)?,
        mc_mesh: method(meshc, "get_sharedMesh", 0)?,
        mc_convex: method(meshc, "get_convex", 0)?,
        mesh_readable: method(mesh, "get_isReadable", 0)?,
        mesh_bounds: method(mesh, "get_bounds", 0)?,
        mesh_vertices: method(mesh, "get_vertices", 0)?,
        mesh_triangles: method(mesh, "get_triangles", 0)?,
        col_raycast: method_sig(collider, "Raycast", &["UnityEngine.Ray", "UnityEngine.RaycastHit&", "System.Single"])?,
        tr_inverse: method(tr, "get_worldToLocalMatrix", 0)?,
        carry_drop_pos: method(carry, "GetSafeDropPosition", 1)?,
        carry_pickup: method(carry, "UserCode_CmdPickup__NetworkIdentity", 1)?,
        carry_drop: method(carry, "UserCode_CmdDrop__NetworkIdentity", 1)?,
        align_pitch: method(pc, "AlignCameraPitchToward", 2)?,
        player_interact: method(player, "get_Interact", 0)?,
        net_id: method(identity, "get_netId", 0)?,
        tr_scale: method(tr, "get_localScale", 0)?,
        sphere_cast: method_sig(
            physics,
            "SphereCast",
            &[
                "UnityEngine.Vector3", "System.Single", "UnityEngine.Vector3", "UnityEngine.RaycastHit&", "System.Single",
                "System.Int32", "UnityEngine.QueryTriggerInteraction",
            ],
        )?,
        check_sphere: method_sig(
            physics,
            "CheckSphere",
            &["UnityEngine.Vector3", "System.Single", "System.Int32", "UnityEngine.QueryTriggerInteraction"],
        )?,
        off_carrier: f(carry, "m_CarrierNetId")?,
        off_floater_radius: f(carry, "floaterRadius")?,
        off_float_colliders: f(carry, "m_Colliders")?,
        t_carry: ty(carry),
        t_identity: ty(identity),
        get_component_in_children: method_sig(comp, "GetComponentInChildren", &["System.Type", "System.Boolean"])?,
        renderer_enabled: method(renderer, "get_enabled", 0)?,
        go_layer: method(go, "get_layer", 0)?,
        ignore_layers: method(physics, "GetIgnoreLayerCollision", 2)?,
        ignore_pair: method(physics, "GetIgnoreCollision", 2).ok(),
        item_names: method(items, "GetItemsNames", 0)?,
        add_item: method(inv, "ServerAddItem", 1)?,
        remove_item: method(inv, "ServerRemoveItem", 2)?,
        item_count: method(inv, "GetItemCount", 1)?,
        entities_singleton: method(ents, "get_Singleton", 0)?,
        gm_singleton: method(gm, "get_singleton", 0)?,
        bm_singleton: method(bm, "get_Singleton", 0)?,
        bm_level_info: method(bm, "get_CurrentLevelInfo", 0)?,
        off_camera: f(bpc, "playerCamera")?,
        off_character: f(pc, "Controller")?,
        off_gravity: f(pc, "m_GravityForce")?,
        off_grounded: f(bpc, "groundedChecker")?,
        off_falling_time: f(grounded, "<FallingTime>k__BackingField")?,
        off_falling_inf: f(grounded, "<FallingToInfinte>k__BackingField")?,
        off_look: f(pc, "lookDirection")?,
        off_boost: f(stats, "m_HasEnergyBoost")?,
        off_boost_timer: f(stats, "m_EnergyBoostTimer")?,
        off_stats: [f(stats, "health")?, f(stats, "m_Stamina")?, f(stats, "m_Oxygen")?, f(stats, "anxiety")?],
        off_invuln: f(stats, "m_HasInvulnerability")?,
        off_spawned: f(ents, "SpawnedEntities")?,
        off_vars: [
            "walkSpeed", "crawlWalkSpeed", "runSpeed", "jogSpeed", "crouchWalkSpeed", "climbSpeed", "ropeClimbSpeed",
            "swimSpeed", "swimSpeedFast", "diveSpeed", "diveSpeedFast", "jumpForce",
        ]
        .iter()
        .map(|n| f(vars, n))
        .collect::<Result<_, _>>()?,
        off_fx: [
            "vignette", "chromatic", "blur", "paniniProjection", "grainNoise", "depthOfField", "cloudedVision",
            "lowresEffect", "vhsEffect", "lensDistortion",
        ]
        .iter()
        .map(|n| f(cam, n))
        .collect::<Result<_, _>>()?,
        off_color_adj: f(cam, "colorAdjustments")?,
        off_post_exposure: f(color_adj, "postExposure")?,
        off_active: f(vol_comp, "active")?,
        off_override: f(vol_param, "m_OverrideState")?,
        off_weight: f(volume, "weight")?,
        off_gm_vhs: f(gm, "vhsEffect")?,
        off_level_post: f(level_params, "levelPostprocess")?,
        off_reset_point: f(level_params, "resetPlayerPoint")?,
        off_spawnpoints: f(level_params, "playerSpawnpoints")?,
        off_tp_points: f(level_params, "teleportPoints")?,
        off_exit_id: f(exit, "endGameID")?,
        off_swap_level: f(swap, "newLevel")?,
        off_tp_dest: f(tp, "teleportPoint")?,
        t_collider: ty(collider),
        t_renderer: ty(renderer),
        k_box: boxc,
        k_sphere: sphere,
        k_capsule: capsule,
        k_mesh: meshc,
        k_character: character,
        boundaries: k("LevelBoundaries")
            .and_then(|b| {
                Ok(Boundaries {
                    t: ty(b),
                    level: method(b, "get_Level", 0)?,
                    off_start: f(b, "startZone")?,
                    off_end: f(b, "endZones")?,
                })
            })
            .ok(),
        t_exit: ty(exit),
        t_swap: ty(swap),
        t_tp: ty(tp),
        t_elevators: ELEVATORS.iter().filter_map(|n| k(n).ok()).map(|c| ty(c)).collect(),
        specials: SPECIAL_TRIGGERS.iter().filter_map(|(ns, n, l)| class(ac, ns, n).ok().map(|c| (ty(c), *l))).collect(),
        level_names: il2cpp::enum_values(k("BackroomsLevel")?),
        pc_update: method(pc, "Update", 0)?,
        cam_update: method(cam, "Update", 0)?,
        begin_camera: method(render_pipeline, "BeginCameraRendering", 2)?,
        take_damage: method(stats, "ServerTakeDamage", 1)?,
        kill_player: method(stats, "CmdKillPlayer", 1)?,
        monster_kill: method(bpc, "PerformMonsterKill", 4)?,
        infinite_falling: method(bpc, "CheckForInfinteFalling", 0)?,
        wear_item: method(inv, "ServerWearItem", 2)?,
        level_unlocked: method(k("BackroomsLevelsAccess")?, "IsLevelUnlocked", 1)?,
        level_unlock: (|| -> Result<LevelUnlock, String> {
            let (dict, info, mgr) = (class(ac, "ITB.Core", "ITBDictionary")?, class(ac, "ITB.Scenario", "BasicLevelInfo")?, k("AchievmentManager")?);
            let u = LevelUnlock {
                get_info: method_sig(dict, "GetLevelInfo", &["BackroomsLevel"])?,
                off_stat: f(info, "m_LevelUnlockStat")?,
                instance: method(mgr, "get_Instance", 0)?,
                set_stat: method_sig(mgr, "SetNewStatValue", &["AchievmentManager.SteamStat", "System.Int32"])?,
                mark: method(mgr, "MarkUnlockedLevelByStat", 1)?,
            };
            // The calls below depend on it: a static SetNewStatValue, an instance MarkUnlockedLevelByStat.
            let shapes = [(u.get_info, true), (u.instance, true), (u.set_stat, true), (u.mark, false)];
            if shapes.iter().any(|&(m, want_static)| il2cpp::is_static(m) != want_static) {
                return Err("forme inattendue des fonctions de deblocage".into());
            }
            Ok(u)
        })()
        .ok(),
    })
}

fn g() -> &'static G {
    G.get().unwrap()
}

// Trampolines to the original functions.
static O_UPDATE: AtomicUsize = AtomicUsize::new(0);
static O_CAM_UPDATE: AtomicUsize = AtomicUsize::new(0);
static O_BEGIN_CAMERA: AtomicUsize = AtomicUsize::new(0);
static O_DAMAGE: AtomicUsize = AtomicUsize::new(0);
static O_KILL: AtomicUsize = AtomicUsize::new(0);
static O_MONSTER_KILL: AtomicUsize = AtomicUsize::new(0);
static O_FALLING: AtomicUsize = AtomicUsize::new(0);
static O_WEAR: AtomicUsize = AtomicUsize::new(0);
static O_UNLOCKED: AtomicUsize = AtomicUsize::new(0);

/// Called once from the init thread, after hudhook initialized MinHook.
pub unsafe fn install() -> Result<(), String> {
    let thread = (il2cpp::api().thread_attach)((il2cpp::api().domain_get)());
    let r = resolve();
    (il2cpp::api().thread_detach)(thread);
    _ = G.set(r.map_err(|e| format!("introuvable dans le jeu : {e}"))?);
    let g = g();
    crate::log(&format!(
        "{} niveaux, {}/{} types de triggers, {}/{} ascenseurs",
        g.level_names.len(),
        g.specials.len(),
        SPECIAL_TRIGGERS.len(),
        g.t_elevators.len(),
        ELEVATORS.len()
    ));
    crate::log(if g.level_unlock.is_some() {
        "deblocage definitif des niveaux : disponible"
    } else {
        "deblocage definitif des niveaux : indisponible dans cette version"
    });
    let hooks: [(M, usize, &AtomicUsize); 9] = [
        (g.pc_update, pc_update as usize, &O_UPDATE),
        (g.cam_update, cam_update as usize, &O_CAM_UPDATE),
        (g.begin_camera, begin_camera as usize, &O_BEGIN_CAMERA),
        (g.take_damage, take_damage as usize, &O_DAMAGE),
        (g.kill_player, kill_player as usize, &O_KILL),
        (g.monster_kill, monster_kill as usize, &O_MONSTER_KILL),
        (g.infinite_falling, infinite_falling as usize, &O_FALLING),
        (g.wear_item, wear_item as usize, &O_WEAR),
        (g.level_unlocked, level_unlocked as usize, &O_UNLOCKED),
    ];
    for (target, detour, orig) in hooks {
        let h = MhHook::new(target.0 as _, detour as _).map_err(|e| format!("hook: {e:?}"))?;
        orig.store(h.trampoline() as usize, Relaxed);
        h.queue_enable().map_err(|e| format!("hook: {e:?}"))?;
    }
    MH_ApplyQueued().ok().map_err(|e| format!("hook: {e:?}"))
}

unsafe fn teleport(g: &G, ctrl: P, pos: V3, rot: Quat, hold_for_ground: bool) {
    if g.teleport_hold {
        call!(g.teleport, fn(P, V3, Quat, bool), ctrl, pos, rot, hold_for_ground);
    } else {
        call!(g.teleport, fn(P, V3, Quat), ctrl, pos, rot);
    }
}

unsafe fn local_controller() -> P {
    call!(g().player_controller, fn() -> P)
}

unsafe fn local_stats() -> P {
    call!(g().player_stats, fn() -> P)
}

unsafe extern "C-unwind" fn pc_update(this: P, mi: usize) {
    let orig: unsafe extern "C-unwind" fn(P, usize) = std::mem::transmute(O_UPDATE.load(Relaxed));
    orig(this, mi);
    if !this.is_null() && this == local_controller() {
        tick(g(), this);
    }
}

// Visual overrides go after the camera's own Update, which animates the same effects.
unsafe extern "C-unwind" fn cam_update(this: P, mi: usize) {
    let orig: unsafe extern "C-unwind" fn(P, usize) = std::mem::transmute(O_CAM_UPDATE.load(Relaxed));
    orig(this, mi);
    if !this.is_null() && this == call!(g().player_camera, fn() -> P) {
        visuals(g(), this);
    }
}

// HDRP calls this right before rendering a camera, once animation and head bob moved it for the
// frame: hitbox lines built here sit exactly on the image (Update-time matrices lag behind).
unsafe extern "C-unwind" fn begin_camera(context: usize, cam: P, mi: usize) {
    let orig: unsafe extern "C-unwind" fn(usize, P, usize) = std::mem::transmute(O_BEGIN_CAMERA.load(Relaxed));
    orig(context, cam, mi);
    let mut guard = lock();
    let s = &mut *guard;
    if !cam.is_null() && cam as usize == s.camera && alive(cam) {
        draw(g(), s, cam);
        s.camera_hook_drew = true;
    }
}

unsafe extern "C-unwind" fn take_damage(this: P, dmg: f32, mi: usize) {
    if GOD.load(Relaxed) && this == local_stats() {
        return;
    }
    let orig: unsafe extern "C-unwind" fn(P, f32, usize) = std::mem::transmute(O_DAMAGE.load(Relaxed));
    orig(this, dmg, mi)
}

unsafe extern "C-unwind" fn kill_player(this: P, sfx: bool, mi: usize) {
    if GOD.load(Relaxed) && this == local_stats() {
        return;
    }
    let orig: unsafe extern "C-unwind" fn(P, bool, usize) = std::mem::transmute(O_KILL.load(Relaxed));
    orig(this, sfx, mi)
}

unsafe extern "C-unwind" fn monster_kill(this: P, monster: P, entity: i32, idx: i32, dmg: f32, mi: usize) {
    if GOD.load(Relaxed) && this == local_controller() {
        return;
    }
    let orig: unsafe extern "C-unwind" fn(P, P, i32, i32, f32, usize) =
        std::mem::transmute(O_MONSTER_KILL.load(Relaxed));
    orig(this, monster, entity, idx, dmg, mi)
}

// The game's anti-fall: >5 s airborne with no ground below teleports you to the level's
// resetPlayerPoint (or the last grounded position).
unsafe extern "C-unwind" fn infinite_falling(this: P, mi: usize) {
    if (NOCLIP.load(Relaxed) || NO_FALL_TP.load(Relaxed)) && this == local_controller() {
        return;
    }
    let orig: unsafe extern "C-unwind" fn(P, usize) = std::mem::transmute(O_FALLING.load(Relaxed));
    orig(this, mi)
}

unsafe extern "C-unwind" fn wear_item(this: P, item: P, amount: f32, mi: usize) {
    if INF_ITEMS.load(Relaxed) {
        return;
    }
    let orig: unsafe extern "C-unwind" fn(P, P, f32, usize) = std::mem::transmute(O_WEAR.load(Relaxed));
    orig(this, item, amount, mi)
}

// Static BackroomsLevelsAccess.IsLevelUnlocked(BackroomsLevel): only answers, saves nothing.
unsafe extern "C-unwind" fn level_unlocked(level: i32, mi: usize) -> bool {
    if UNLOCK_LEVELS.load(Relaxed) {
        return true;
    }
    let orig: unsafe extern "C-unwind" fn(i32, usize) -> bool = std::mem::transmute(O_UNLOCKED.load(Relaxed));
    orig(level, mi)
}

fn key(vk: u16) -> bool {
    unsafe { GetAsyncKeyState(vk as i32) as u16 & 0x8000 != 0 }
}

/// Key by physical position (scan code), so WASD is ZQSD on AZERTY like in the game.
fn key_at(scan: u32) -> bool {
    key(unsafe { MapVirtualKeyW(scan, MAPVK_VSC_TO_VK) } as u16)
}

const SC_W: u32 = 0x11;
const SC_A: u32 = 0x1E;
const SC_S: u32 = 0x1F;
const SC_D: u32 = 0x20;

unsafe fn transform_pos(g: &G, comp: P) -> Option<V3> {
    if !alive(comp) {
        return None;
    }
    let tr = call!(g.comp_transform, fn(P) -> P, comp);
    alive(tr).then(|| call!(g.tr_get_pos, fn(P) -> V3, tr))
}

unsafe fn name_of(g: &G, obj: P) -> String {
    il2cpp::string(call!(g.obj_name, fn(P) -> P, obj)).replace("(Clone)", "").trim().to_string()
}

unsafe fn find(g: &G, ty: P) -> Vec<P> {
    il2cpp::array(call!(g.find_objects, fn(P, i32) -> P, ty, 0))
}

fn level_name(g: &G, id: i32) -> String {
    g.level_names.iter().find(|(v, _)| *v == id).map_or(id.to_string(), |(_, n)| n.clone())
}

/// Item name -> count for every known item the inventory holds.
unsafe fn inventory(g: &G, inv: P, names: &[String]) -> Vec<(String, i32)> {
    names
        .iter()
        .map(|n| (n.clone(), call!(g.item_count, fn(P, P) -> i32, inv, il2cpp::new_string(n))))
        .filter(|(_, c)| *c > 0)
        .collect()
}

unsafe fn scan_points(g: &G) -> Vec<Point> {
    let mut v = Vec::new();
    let mut add = |label: String, pos: Option<V3>| {
        if let Some(pos) = pos {
            v.push(Point { label, pos });
        }
    };
    let mgr = call!(g.bm_singleton, fn() -> P);
    let info = if alive(mgr) { call!(g.bm_level_info, fn(P) -> P, mgr) } else { std::ptr::null_mut() };
    if !info.is_null() {
        add("Point de reset anti-chute".into(), transform_pos(g, rd(info, g.off_reset_point)));
        for (i, t) in il2cpp::array(rd(info, g.off_spawnpoints)).into_iter().enumerate() {
            add(format!("Spawn #{}", i + 1), transform_pos(g, t));
        }
        for (i, t) in il2cpp::array(rd(info, g.off_tp_points)).into_iter().enumerate() {
            add(format!("Point de TP #{}", i + 1), transform_pos(g, t));
        }
    }
    for (lb, b) in g.boundaries.iter().flat_map(|lb| find(g, lb.t).into_iter().map(move |b| (lb, b))) {
        let level = level_name(g, call!(lb.level, fn(P) -> i32, b));
        let start: P = rd(b, lb.off_start);
        if !start.is_null() {
            add(format!("Debut {level}"), field_of(start, "center").ok().and_then(|o| transform_pos(g, rd(start, o))));
        }
        for z in il2cpp::array(rd(b, lb.off_end)).into_iter().filter(|z| !z.is_null()) {
            let unlock = field_of(z, "unlockLevel").map_or(String::new(), |o| level_name(g, rd(z, o)));
            add(format!("Fin {level} -> {unlock}"), field_of(z, "center").ok().and_then(|o| transform_pos(g, rd(z, o))));
        }
    }
    for c in find(g, g.t_exit) {
        add(format!("Sortie (fin #{})", rd::<i32>(c, g.off_exit_id)), transform_pos(g, c));
    }
    for c in find(g, g.t_swap) {
        add(format!("Passage -> {}", level_name(g, rd(c, g.off_swap_level))), transform_pos(g, c));
    }
    for c in find(g, g.t_tp) {
        let name = name_of(g, c);
        add(format!("Teleporteur {name}"), transform_pos(g, c));
        let dest: P = rd(c, g.off_tp_dest);
        if alive(dest) {
            add(format!("Destination de {name}"), Some(call!(g.tr_get_pos, fn(P) -> V3, dest)));
        }
    }
    for &t in &g.t_elevators {
        for c in find(g, t) {
            add(format!("Ascenseur {}", name_of(g, c)), transform_pos(g, c));
        }
    }
    v
}

// The game's own names for its invisible walls (BoundsCollider, InvisibleWall, LiftBoundsCollider,
// LimitExitCollider...). A missing renderer proves nothing here: most of the world is drawn through
// Entities, and its collision sits on bare objects (PROPS_COLLIDERS_CACHE/*_cacheCollider).
fn invisible_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["invisible", "bounds", "limitexit"].iter().any(|k| n.contains(k))
}

/// What a collider is to the player (ignored, wall, invisible wall, trigger zone) and its shape.
/// None when the shape needs raycast sampling and this scan's budget for it is spent.
unsafe fn describe(g: &G, col: P, k: P, player: Option<(P, i32)>, budget: &mut u32) -> Option<Info> {
    let go = call!(g.comp_gameobject, fn(P) -> P, col);
    let layer = call!(g.go_layer, fn(P) -> i32, go);
    // shortcut: ignores Collider.excludeLayers (stripped from this build) and layer changes after the scan.
    let touches = player.is_none_or(|(cc, own)| {
        !call!(g.ignore_layers, fn(i32, i32) -> bool, own, layer)
            && !g.ignore_pair.is_some_and(|m| call!(m, fn(P, P) -> bool, cc, col))
    });
    let name = name_of(g, go);
    let mut label = String::new();
    let kind = if !touches {
        Kind::Ignored
    } else if call!(g.col_trigger, fn(P) -> bool, col) {
        // Component.GetComponent: `this` must be a Component (the collider), never its GameObject.
        let special = g.specials.iter().find(|(t, _)| !call!(g.get_component, fn(P, P) -> P, col, *t).is_null());
        label = special.map_or(name.clone(), |(_, l)| format!("{l} ({name})"));
        if special.is_some() { Kind::Special } else { Kind::Trigger }
    } else if invisible_name(&name) {
        // Unless something visible is drawn on it after all.
        let r = call!(g.get_component_in_children, fn(P, P, bool) -> P, col, g.t_renderer, false);
        if !r.is_null() && call!(g.renderer_enabled, fn(P) -> bool, r) { Kind::Solid } else { Kind::Invisible }
    } else {
        Kind::Solid
    };
    let shape = if kind == Kind::Ignored { Shape::default() } else { shape(g, col, k, budget)? };
    Some(Info { kind, label, shape })
}

const AXES: [V3; 3] = [V3 { x: 1.0, y: 0.0, z: 0.0 }, V3 { x: 0.0, y: 1.0, z: 0.0 }, V3 { x: 0.0, y: 0.0, z: 1.0 }];
const MAX_MESH_TRIS: usize = 60_000;
const MAX_FILLED_TRIS: usize = 2_000;

unsafe fn shape(g: &G, col: P, k: P, budget: &mut u32) -> Option<Shape> {
    let v3 = |m: M| call!(m, fn(P) -> V3, col);
    let f = |m: M| call!(m, fn(P) -> f32, col);
    Some(if k == g.k_box {
        box_shape(v3(g.box_center), v3(g.box_size).scale(0.5))
    } else if k == g.k_sphere {
        let (c, r) = (v3(g.sphere_center), f(g.sphere_radius));
        let mut edges = Vec::new();
        for i in 0..3 {
            arc(&mut edges, c, AXES[i], AXES[(i + 1) % 3], r, 0.0, TAU);
        }
        Shape { edges, ..Default::default() }
    } else if k == g.k_capsule {
        let dir = call!(g.capsule_direction, fn(P) -> i32, col);
        capsule_shape(v3(g.capsule_center), f(g.capsule_radius), f(g.capsule_height), dir)
    } else if k == g.k_mesh {
        return mesh_shape(g, col, budget);
    } else {
        Shape::default() // terrain / wheel colliders: not drawn
    })
}

unsafe fn mesh_shape(g: &G, col: P, budget: &mut u32) -> Option<Shape> {
    let mesh = call!(g.mc_mesh, fn(P) -> P, col);
    if !alive(mesh) {
        return Some(Shape::default());
    }
    // Physics uses a convex mesh's hull, not its triangles: that one gets sampled too.
    if !call!(g.mc_convex, fn(P) -> bool, col) && call!(g.mesh_readable, fn(P) -> bool, mesh) {
        let idx: Vec<i32> = il2cpp::values(call!(g.mesh_triangles, fn(P) -> P, mesh));
        if idx.len() <= 3 * MAX_MESH_TRIS {
            let verts: Vec<V3> = il2cpp::values(call!(g.mesh_vertices, fn(P) -> P, mesh));
            return Some(Shape { edges: outline(&verts, &idx), faces: faces(&verts, &idx), approx: false });
        }
    }
    if *budget == 0 {
        return None;
    }
    *budget -= 1;
    let b = call!(g.mesh_bounds, fn(P) -> Bounds, mesh);
    let shape = sampled_shape(g, col, b);
    // Nothing hit (shouldn't happen): its bounding box, flagged as approximate.
    Some(if shape.edges.is_empty() { Shape { approx: true, ..box_shape(b.center, b.extents) } } else { shape })
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Ray {
    origin: V3,
    dir: V3,
}

/// UnityEngine.RaycastHit: point and distance are read; the rest just needs room.
#[repr(C)]
#[derive(Default)]
struct Hit {
    point: V3,
    normal: V3,
    face: u32,
    distance: f32,
    rest: [u32; 24],
}

/// Raycasts this one collider (Collider.Raycast), so what gets drawn is what PhysX collides with.
unsafe fn sampled_shape(g: &G, col: P, local: Bounds) -> Shape {
    let tr = call!(g.comp_transform, fn(P) -> P, col);
    let (to_world, to_local) = (call!(g.tr_matrix, fn(P) -> Mat4, tr), call!(g.tr_inverse, fn(P) -> Mat4, tr));
    sample_shape(local, |from, to| {
        let (a, b) = (to_world.apply(from), to_world.apply(to));
        let len = a.dist(b);
        let mut hit = Hit::default();
        let ray = Ray { origin: a, dir: b.sub(a).scale(1.0 / len) };
        (len > 0.0 && call!(g.col_raycast, fn(P, Ray, *mut Hit, f32) -> bool, col, ray, &mut hit, len))
            .then(|| to_local.apply(hit.point))
    })
}

const ANGLES: usize = 32;
const MAX_SLICES: usize = 12;

/// Shape of a collider that can only be queried, drawn like a lampshade: horizontal slices (local
/// Y), each traced by rays shot from all around toward the axis (outer contour) and from the axis
/// outward (inner contour: a float's hole), joined by a few vertical lines.
/// `cast(from, to)` gives the first hit.
fn sample_shape(local: Bounds, mut cast: impl FnMut(V3, V3) -> Option<V3>) -> Shape {
    let e = local.extents;
    let reach = (e.x * e.x + e.z * e.z).sqrt() * 1.1 + 0.01;
    // About one slice per eighth of the width: a flat float gets ~6, a tall pillar the max.
    let slices = (2.0 * e.y / (reach / 8.0)).ceil().clamp(2.0, MAX_SLICES as f32) as usize;
    let step = 2.0 * e.y / slices as f32;
    let mut rings: [Vec<Vec<Option<V3>>>; 2] = Default::default();
    for k in 0..slices {
        let axis = V3 { y: local.center.y - e.y + step * (k as f32 + 0.5), ..local.center };
        for (inward, side) in rings.iter_mut().enumerate() {
            side.push(
                (0..ANGLES)
                    .map(|i| {
                        let a = TAU * i as f32 / ANGLES as f32;
                        let rim = axis.add_scaled(V3 { x: a.cos(), y: 0.0, z: a.sin() }, reach);
                        let (from, to) = if inward == 0 { (rim, axis) } else { (axis, rim) };
                        // A ray starting inside the shape may report its own origin: not a surface.
                        cast(from, to).filter(|p| p.dist(from) > 1e-3)
                    })
                    .collect(),
            );
        }
    }
    // Hits farther apart than this are on different surfaces: don't join them.
    let gap = 2.0 * (TAU * reach / ANGLES as f32).max(step);
    let near = |a: Option<V3>, b: Option<V3>| match (a, b) {
        (Some(a), Some(b)) if a.dist(b) < gap => Some((a, b)),
        _ => None,
    };
    let mut shape = Shape::default();
    for side in &rings {
        for k in 0..slices {
            for i in 0..ANGLES {
                let j = (i + 1) % ANGLES;
                shape.edges.extend(near(side[k][i], side[k][j]));
                let Some(up) = side.get(k + 1) else { continue };
                if i % (ANGLES / 8) == 0 {
                    shape.edges.extend(near(side[k][i], up[i]));
                }
                if let (Some((a, b)), Some((c, d))) = (near(side[k][i], side[k][j]), near(up[i], up[j])) {
                    shape.faces.extend([[a, b, d], [a, d, c]]);
                }
            }
        }
    }
    shape
}

/// The triangles to fill, unless there are too many to draw every frame.
fn faces(verts: &[V3], idx: &[i32]) -> Vec<[V3; 3]> {
    if idx.len() > 3 * MAX_FILLED_TRIS {
        return Vec::new();
    }
    idx.chunks_exact(3).filter_map(|t| tri(verts, t)).map(|t| t.map(|i| verts[i])).collect()
}

const EDGES: [(usize, usize); 12] =
    [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)];
// Corners of each face in order around it (corner index bits: 1 = +x, 2 = +y, 4 = +z).
const QUADS: [[usize; 4]; 6] = [[0, 2, 6, 4], [1, 3, 7, 5], [0, 1, 5, 4], [2, 3, 7, 6], [0, 1, 3, 2], [4, 5, 7, 6]];

fn box_shape(center: V3, half: V3) -> Shape {
    let corner = |i: usize| {
        let s = |bit: usize| if i & bit != 0 { 1.0 } else { -1.0 };
        V3 { x: center.x + half.x * s(1), y: center.y + half.y * s(2), z: center.z + half.z * s(4) }
    };
    Shape {
        edges: EDGES.iter().map(|&(a, b)| (corner(a), corner(b))).collect(),
        faces: QUADS
            .iter()
            .flat_map(|q| [[corner(q[0]), corner(q[1]), corner(q[2])], [corner(q[0]), corner(q[2]), corner(q[3])]])
            .collect(),
        approx: false,
    }
}

/// Circle arc of radius r around c in the plane (u, v), from angle a0 to a1, in 15° steps.
fn arc(edges: &mut Vec<(V3, V3)>, c: V3, u: V3, v: V3, r: f32, a0: f32, a1: f32) {
    let n = ((a1 - a0) / (TAU / 24.0)).ceil().max(1.0) as usize;
    let p = |i: usize| {
        let t = a0 + (a1 - a0) * i as f32 / n as f32;
        c.add_scaled(u, r * t.cos()).add_scaled(v, r * t.sin())
    };
    edges.extend((0..n).map(|i| (p(i), p(i + 1))));
}

/// Capsule along local axis `dir` (0 = X, 1 = Y, 2 = Z): end rings, side lines and the two caps.
fn capsule_shape(c: V3, r: f32, height: f32, dir: i32) -> Shape {
    let d = dir.clamp(0, 2) as usize;
    let (a, u, v) = (AXES[d], AXES[(d + 1) % 3], AXES[(d + 2) % 3]);
    let half = (height / 2.0 - r).max(0.0);
    let (top, bottom) = (c.add_scaled(a, half), c.add_scaled(a, -half));
    let mut edges = Vec::new();
    for end in [top, bottom] {
        arc(&mut edges, end, u, v, r, 0.0, TAU);
    }
    for side in [u, v, u.scale(-1.0), v.scale(-1.0)] {
        edges.push((top.add_scaled(side, r), bottom.add_scaled(side, r)));
    }
    for side in [u, v] {
        arc(&mut edges, top, side, a, r, 0.0, PI);
        arc(&mut edges, bottom, side, a, r, PI, TAU);
    }
    Shape { edges, ..Default::default() }
}

/// Vertex indices of one triangle, None if any is out of range.
fn tri(verts: &[V3], t: &[i32]) -> Option<[usize; 3]> {
    let t = [t[0] as usize, t[1] as usize, t[2] as usize];
    t.iter().all(|&i| i < verts.len()).then_some(t)
}

/// Edges worth drawing on a mesh: borders and creases between faces at an angle. The diagonal of a
/// flat quad is dropped, so a wall reads as a rectangle instead of a pile of triangles.
fn outline(verts: &[V3], idx: &[i32]) -> Vec<(V3, V3)> {
    // Weld the vertices meshes split per normal/uv, so neighbouring faces share their edges.
    let mut welded = HashMap::new();
    let id: Vec<usize> = verts
        .iter()
        .map(|v| {
            let n = welded.len();
            *welded.entry([v.x, v.y, v.z].map(|c| (c * 1000.0).round() as i32)).or_insert(n)
        })
        .collect();
    // (ends, normal of the first face, face count, crease)
    let mut edges: HashMap<(usize, usize), (V3, V3, V3, u32, bool)> = HashMap::new();
    for t in idx.chunks_exact(3).filter_map(|t| tri(verts, t)) {
        let [a, b, c] = t.map(|i| verts[i]);
        let n = b.sub(a).cross(c.sub(a));
        let len = n.dot(n).sqrt();
        if len < 1e-9 {
            continue;
        }
        let n = n.scale(1.0 / len);
        for (i, j) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            let e = edges.entry((id[i].min(id[j]), id[i].max(id[j]))).or_insert((verts[i], verts[j], n, 0, false));
            e.3 += 1;
            e.4 |= e.2.dot(n) < 0.97; // faces more than ~14° apart
        }
    }
    edges.into_values().filter(|e| e.3 != 2 || e.4).map(|e| (e.0, e.1)).collect()
}

/// Distance from a point to an axis-aligned box.
fn aabb_dist(b: &Bounds, p: V3) -> f32 {
    let d = |c: f32, e: f32, v: f32| ((v - c).abs() - e).max(0.0);
    let (x, y, z) = (d(b.center.x, b.extents.x, p.x), d(b.center.y, b.extents.y, p.y), d(b.center.z, b.extents.z, p.z));
    (x * x + y * y + z * z).sqrt()
}

unsafe fn scan_colliders(g: &G, s: &mut State, ctrl: P, pos: V3) {
    let cc = rd::<P>(ctrl, g.off_character);
    let player = alive(cc).then(|| (cc, call!(g.go_layer, fn(P) -> i32, call!(g.comp_gameobject, fn(P) -> P, cc))));
    let radius = s.collider_radius * 1.5;
    // ~1000 raycasts per sampled shape: spread them over scans instead of one long hitch.
    let mut budget = 6;
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for c in find(g, g.t_collider) {
        let k = (il2cpp::api().object_get_class)(c);
        if k == g.k_character || !call!(g.col_enabled, fn(P) -> bool, c) {
            continue;
        }
        let key = (c as usize, rd::<usize>(c, 0x10));
        seen.insert(key);
        let bounds = call!(g.col_bounds, fn(P) -> Bounds, c);
        if aabb_dist(&bounds, pos) > radius {
            continue;
        }
        let info = match s.infos.get(&key) {
            Some(info) => info.clone(),
            None => match describe(g, c, k, player, &mut budget) {
                Some(info) => s.infos.entry(key).or_insert(Arc::new(info)).clone(),
                None => continue, // sampling budget spent: next scan
            },
        };
        if info.kind != Kind::Ignored && !info.shape.edges.is_empty() {
            out.push(Col { obj: c as usize, info, bounds });
        }
    }
    s.infos.retain(|k, _| seen.contains(k));
    s.collider_count = out.len();
    s.approx_count = out.iter().filter(|c| c.info.shape.approx).count();
    s.colliders = out;
}

/// Clip-space w is the view depth: geometry closer than this gets cut at it instead of dropped.
const NEAR_W: f32 = 0.05;
const MAX_LINES: usize = 40_000;

/// The part of a clip-space polygon (or segment) in front of the camera, in NDC.
fn clip_front(pts: &[[f32; 4]]) -> Vec<[f32; 2]> {
    let ndc = |p: [f32; 4]| [p[0] / p[3], p[1] / p[3]];
    let mut out = Vec::new();
    for (i, &a) in pts.iter().enumerate() {
        let b = pts[(i + 1) % pts.len()];
        if a[3] > NEAR_W {
            out.push(ndc(a));
        }
        if (a[3] > NEAR_W) != (b[3] > NEAR_W) {
            let t = (NEAR_W - a[3]) / (b[3] - a[3]);
            out.push(ndc([0, 1, 2, 3].map(|k| a[k] + (b[k] - a[k]) * t)));
        }
    }
    out
}

/// Rebuilds this frame's overlay: hitboxes and the float drop preview.
unsafe fn draw(g: &G, s: &mut State, cam: P) {
    s.lines.clear();
    s.polys.clear();
    s.labels.clear();
    let hitboxes = s.show_colliders || s.show_triggers;
    let floats = s.show_float && !s.float_cols.is_empty();
    let trail = s.show_trail && s.trail.len() > 1;
    if !hitboxes && s.drop.is_none() && !floats && !trail && s.trials.is_empty() {
        return;
    }
    let eye = call!(g.tr_get_pos, fn(P) -> V3, call!(g.comp_transform, fn(P) -> P, cam));
    let vp = call!(g.cam_proj, fn(P) -> Mat4, cam).mul(&call!(g.cam_view, fn(P) -> Mat4, cam));
    if hitboxes {
        draw_colliders(g, s, eye, &vp);
    }
    if let Some(d) = s.drop {
        draw_drop(s, &vp, d);
    }
    if floats {
        draw_floats(g, s, &vp);
    }
    if trail {
        draw_trail(s, &vp);
    }
    draw_trials(s, &vp);
}

/// Where the float ended up after the last test drops, colored by how each went; every spot tried,
/// from red (never works) to green (always works); and the best line up: a ring where to stand, an
/// arrow where to face, a cross where to aim for the horn.
fn draw_trials(s: &mut State, vp: &Mat4) {
    for t in s.trials.iter().rev().take(40) {
        let mut edges = Vec::new();
        for i in 0..3 {
            arc(&mut edges, t.float_pos, AXES[i], AXES[(i + 1) % 3], 0.08, 0.0, TAU);
        }
        add_lines(&mut s.lines, vp, edges, t.outcome.color());
    }
    let (from, to) = s.run_slots;
    let Some(a) = s.slots.get(from) else { return };
    let b = to.and_then(|i| s.slots.get(i));
    let n = spot_count(a, b);
    for (i, &(tries, ok)) in spot_stats(&s.trials, n).iter().enumerate().filter(|(_, st)| st.0 > 0) {
        let rate = ok as f32 / tries as f32;
        let mut edges = Vec::new();
        arc(&mut edges, spot_at(a, b, i, n).pos, AXES[0], AXES[2], 0.025, 0.0, TAU);
        add_lines(&mut s.lines, vp, edges, [1.0 - rate, rate, 0.2, 1.0]);
    }
    let Some(l) = lineup(s) else { return };
    let white = [1.0; 4];
    let (pos, yaw) = (l.saved.pos, l.yaw.to_radians());
    let fwd = V3 { x: yaw.sin(), y: 0.0, z: yaw.cos() };
    let right = V3 { x: fwd.z, y: 0.0, z: -fwd.x };
    let tip = pos.add_scaled(fwd, 1.0);
    let back = tip.add_scaled(fwd, -0.15);
    let mut edges = vec![(pos, tip), (tip, back.add_scaled(right, 0.08)), (tip, back.add_scaled(right, -0.08))];
    arc(&mut edges, pos, AXES[0], AXES[2], 0.12, 0.0, TAU);
    add_lines(&mut s.lines, vp, edges, white);
    if let Some(p) = vp.project(tip) {
        s.labels.push((p, format!("line up : {}/{}", l.own.1, l.own.0), white));
    }
    if let Some(g) = l.grab {
        add_lines(&mut s.lines, vp, AXES.iter().map(|&ax| (g.add_scaled(ax, -0.05), g.add_scaled(ax, 0.05))), white);
        if let Some(p) = vp.project(g) {
            s.labels.push((p, "vise ici".into(), white));
        }
    }
}

/// World-space segments to overlay lines, minus what's behind the camera.
fn add_lines(lines: &mut Vec<Line>, vp: &Mat4, edges: impl IntoIterator<Item = (V3, V3)>, color: [f32; 4]) {
    for (a, b) in edges {
        if let [a, b, ..] = clip_front(&[vp.point(a), vp.point(b)])[..] {
            lines.push(Line { a, b, color });
        }
    }
}

/// The float as it would appear if dropped now, and a line down to where it would land.
fn draw_drop(s: &mut State, vp: &Mat4, d: DropInfo) {
    let color = d.color();
    let mut edges = Vec::new();
    for i in 0..3 {
        arc(&mut edges, d.pos, AXES[i], AXES[(i + 1) % 3], d.radius, 0.0, TAU);
    }
    if let Some(land) = d.land {
        edges.push((d.pos, land));
        arc(&mut edges, land, AXES[0], AXES[2], d.radius, 0.0, TAU);
    }
    add_lines(&mut s.lines, vp, edges, color);
    if let Some(p) = vp.project(d.pos) {
        s.labels.push((p, format!("DROP : {}", d.label()), color));
    }
}

pub const FLOAT_COLOR: [f32; 4] = [1.0, 0.35, 0.9, 1.0];
const TRAIL_COLOR: [f32; 4] = [1.0, 0.75, 1.0, 0.9];

/// The floats lying around, through walls (not the carried one: it fills the screen already).
unsafe fn draw_floats(g: &G, s: &mut State, vp: &Mat4) {
    for (float, (col, _), info) in &s.float_cols {
        let col = *col as P;
        if *float == s.carried || !alive(col) || !call!(g.col_enabled, fn(P) -> bool, col) || s.lines.len() >= MAX_LINES {
            continue;
        }
        let model = call!(g.tr_matrix, fn(P) -> Mat4, call!(g.comp_transform, fn(P) -> P, col));
        let [r, gr, b, _] = FLOAT_COLOR;
        let color = if info.kind == Kind::Trigger { [r, gr, b, 0.25] } else { FLOAT_COLOR };
        add_lines(&mut s.lines, vp, info.shape.edges.iter().map(|&(a, b)| (model.apply(a), model.apply(b))), color);
    }
    if let (Some(p), Some(d)) = (s.float_top_pos.and_then(|p| vp.project(p)), s.float_top) {
        s.labels.push((p, format!("sommet {}", top_label(d)), FLOAT_COLOR));
    }
}

/// Where the last dropped float went since it left your hands.
fn draw_trail(s: &mut State, vp: &Mat4) {
    let start = s.trail[0];
    let mut edges: Vec<(V3, V3)> = s.trail.windows(2).map(|w| (w[0], w[1])).collect();
    for i in 0..3 {
        arc(&mut edges, start, AXES[i], AXES[(i + 1) % 3], 0.1, 0.0, TAU);
    }
    add_lines(&mut s.lines, vp, edges, TRAIL_COLOR);
    if let Some(p) = vp.project(start) {
        s.labels.push((p, "lachee ici".into(), TRAIL_COLOR));
    }
}

/// "+8 cm": the float's top relative to the ground under the player, rounded to the centimetre.
pub fn top_label(d: f32) -> String {
    format!("{:+} cm", (d * 100.0).round() as i32)
}

/// The float that was carried last frame and isn't anymore (dropped, or swapped for another).
fn just_dropped(before: usize, now: usize) -> Option<usize> {
    (before != 0 && before != now).then_some(before)
}

/// Adds a point to a path unless it barely moved since the last one.
fn extend_trail(trail: &mut Vec<V3>, p: V3) {
    if trail.last().is_none_or(|l| l.dist(p) > 0.01) {
        trail.push(p);
    }
}

/// Game seconds of path recorded after a drop: an ejection or a fall plays out well within it.
const TRAIL_SECS: f32 = 5.0;

/// The colliders of the floats around, plus the last one dropped wherever it went, with their
/// shapes. A shape is only sampled while its collider is enabled (a carried float's may not be).
unsafe fn scan_float_colliders(g: &G, s: &mut State, here: V3) {
    // ~1000 raycasts per sampled shape, like scan_colliders.
    let mut budget = 2;
    let mut out = Vec::new();
    for &float in &s.floats {
        let f = float as P;
        let near = float == s.trail_float || transform_pos(g, f).is_some_and(|p| p.dist(here) < s.collider_radius);
        if !near {
            continue;
        }
        for col in il2cpp::array(rd(f, g.off_float_colliders)) {
            if !alive(col) {
                continue;
            }
            let key = (col as usize, rd::<usize>(col, 0x10));
            let info = match s.float_shapes.get(&key) {
                Some(info) => info.clone(),
                None if call!(g.col_enabled, fn(P) -> bool, col) => {
                    let k = (il2cpp::api().object_get_class)(col);
                    let Some(sh) = shape(g, col, k, &mut budget) else { continue };
                    let kind = if call!(g.col_trigger, fn(P) -> bool, col) { Kind::Trigger } else { Kind::Solid };
                    s.float_shapes.entry(key).or_insert(Arc::new(Info { kind, label: String::new(), shape: sh })).clone()
                },
                None => continue,
            };
            out.push((float, key, info));
        }
    }
    s.float_shapes.retain(|k, _| out.iter().any(|(_, key, _)| key == k));
    s.float_cols = out;
}

/// Highest point of the last dropped float's solid colliders, in world space.
unsafe fn float_top(g: &G, s: &State) -> Option<V3> {
    let mut best: Option<(V3, P)> = None;
    for (float, (col, _), info) in &s.float_cols {
        let col = *col as P;
        if *float != s.trail_float || info.kind != Kind::Solid || !alive(col) || !call!(g.col_enabled, fn(P) -> bool, col) {
            continue;
        }
        let model = call!(g.tr_matrix, fn(P) -> Mat4, call!(g.comp_transform, fn(P) -> P, col));
        for p in info.shape.edges.iter().flat_map(|e| [e.0, e.1]) {
            let w = model.apply(p);
            if best.is_none_or(|(b, _)| w.y > b.y) {
                best = Some((w, col));
            }
        }
    }
    // A sampled shape stops a bit under the real top: one ray straight down on that spot settles it.
    let (top, col) = best?;
    let mut hit = Hit::default();
    let ray = Ray { origin: V3 { y: top.y + 1.0, ..top }, dir: V3 { x: 0.0, y: -1.0, z: 0.0 } };
    Some(if call!(g.col_raycast, fn(P, Ray, *mut Hit, f32) -> bool, col, ray, &mut hit, 2.0) { hit.point } else { top })
}

/// Height of the ground right under the player (the roof, once up there), None in the air.
unsafe fn ground_y(g: &G, cc: P, player_layer: i32) -> Option<f32> {
    let b = call!(g.col_bounds, fn(P) -> Bounds, cc);
    let mask = collision_mask(g, player_layer, player_layer);
    let mut hit = Hit::default();
    let down = V3 { x: 0.0, y: -1.0, z: 0.0 };
    // From the capsule's centre: a cast starting inside the ground would miss it.
    call!(g.sphere_cast, fn(V3, f32, V3, *mut Hit, f32, i32, i32) -> bool, b.center, 0.1, down, &mut hit, b.extents.y + 0.5, mask, IGNORE_TRIGGERS)
        .then_some(hit.point.y)
}

/// Follows the last float we dropped: its path for TRAIL_SECS, and its top against the ground
/// under the player.
unsafe fn track_dropped_float(g: &G, s: &mut State, carried: Option<P>, cc: P, player_layer: Option<i32>) {
    let carried = carried.map_or(0, |f| f as usize);
    if let Some(f) = just_dropped(s.carried, carried) {
        (s.trail_float, s.trail_age) = (f, 0.0);
        s.trail.clear();
    }
    s.carried = carried;
    if s.trail_float != 0 && !alive(s.trail_float as P) {
        s.trail_float = 0; // gone with the level
        s.trail.clear();
    }
    let dropped = s.trail_float != 0 && s.trail_float != carried;
    if dropped && s.trail_age < TRAIL_SECS {
        if let Some(p) = transform_pos(g, s.trail_float as P) {
            extend_trail(&mut s.trail, p);
        }
        s.trail_age += call!(g.delta_time, fn() -> f32);
    }
    s.float_top_pos = if dropped { float_top(g, s) } else { None };
    let ground = player_layer.and_then(|l| ground_y(g, cc, l));
    s.float_top = s.float_top_pos.zip(ground).map(|(t, y)| t.y - y);
}

const IGNORE_TRIGGERS: i32 = 1;

/// Layer mask of what a collider on `layer` collides with, minus the `without` layer.
unsafe fn collision_mask(g: &G, layer: i32, without: i32) -> i32 {
    (0..32)
        .filter(|&l| l != without && !call!(g.ignore_layers, fn(i32, i32) -> bool, layer, l))
        .fold(0i32, |m, l| m | 1 << l)
}

/// Where the carried float would go if dropped now: the game's own GetSafeDropPosition (what
/// CmdDrop uses), then a sphere cast down to where it would land.
unsafe fn drop_preview(g: &G, float: P, player_tr: P, player_layer: i32) -> DropInfo {
    let pos = call!(g.carry_drop_pos, fn(P, P) -> V3, float, player_tr);
    let scale = call!(g.tr_scale, fn(P) -> V3, call!(g.comp_transform, fn(P) -> P, float));
    let radius = rd::<f32>(float, g.off_floater_radius) * scale.x.abs();
    // What the float collides with, minus players: it would land on you instead.
    let layer = call!(g.go_layer, fn(P) -> i32, call!(g.comp_gameobject, fn(P) -> P, float));
    let mask = collision_mask(g, layer, player_layer);
    let inside = call!(g.check_sphere, fn(V3, f32, i32, i32) -> bool, pos, radius * 0.9, mask, IGNORE_TRIGGERS);
    let down = V3 { x: 0.0, y: -1.0, z: 0.0 };
    let mut hit = Hit::default();
    let land = call!(g.sphere_cast, fn(V3, f32, V3, *mut Hit, f32, i32, i32) -> bool, pos, radius, down, &mut hit, 500.0, mask, IGNORE_TRIGGERS)
        .then(|| pos.add_scaled(down, hit.distance));
    DropInfo { pos, radius, land, inside }
}

unsafe fn draw_colliders(g: &G, s: &mut State, eye: V3, vp: &Mat4) {
    let radius = s.collider_radius;
    // Fading with distance makes what's close stand out from what's behind it.
    let fade = |p: V3| (1.0 - p.dist(eye) / radius).clamp(0.15, 1.0);
    for col in &s.colliders {
        let info = &col.info;
        let wanted = match info.kind {
            Kind::Solid => s.show_colliders && s.show_visible_walls,
            Kind::Invisible => s.show_colliders,
            Kind::Trigger | Kind::Special => s.show_triggers,
            Kind::Ignored => false,
        };
        if !wanted || !alive(col.obj as P) || aabb_dist(&col.bounds, eye) > radius || s.lines.len() >= MAX_LINES {
            continue;
        }
        let model = call!(g.tr_matrix, fn(P) -> Mat4, call!(g.comp_transform, fn(P) -> P, col.obj as P));
        let world = |p: V3| model.apply(p);
        let (color, fill) = match info.kind {
            Kind::Solid => ([0.35, 0.75, 1.0, 0.7], 0.0),
            Kind::Invisible => ([1.0, 0.55, 0.0, 1.0], 0.2),
            Kind::Trigger => ([1.0, 0.95, 0.2, 0.85], 0.08),
            _ => ([1.0, 0.2, 0.2, 1.0], 0.16),
        };
        let alpha = |a: f32| [color[0], color[1], color[2], a];
        for &(a, b) in &info.shape.edges {
            let (a, b) = (world(a), world(b));
            let mid = a.add_scaled(b.sub(a), 0.5);
            if mid.dist(eye) > radius {
                continue; // big level meshes: only the part around the player
            }
            if let [a, b, ..] = clip_front(&[vp.point(a), vp.point(b)])[..] {
                s.lines.push(Line { a, b, color: alpha(color[3] * fade(mid)) });
            }
        }
        if fill > 0.0 {
            for face in &info.shape.faces {
                let face = face.map(world);
                let center = face[0].add_scaled(face[1], 1.0).add_scaled(face[2], 1.0).scale(1.0 / 3.0);
                if center.dist(eye) > radius {
                    continue;
                }
                let pts = clip_front(&face.map(|p| vp.point(p)));
                if pts.len() >= 3 {
                    s.polys.push(Poly { pts, color: alpha(fill * fade(center)) });
                }
            }
        }
        // Names only up close, or they pile up into an unreadable mess.
        if !info.label.is_empty() && col.bounds.center.dist(eye) < radius.min(15.0) {
            if let Some(p) = vp.project(col.bounds.center) {
                s.labels.push((p, info.label.clone(), alpha(1.0)));
            }
        }
    }
}

/// How far E reaches (PlayerInteract.interactDistance).
unsafe fn interact_reach(g: &G) -> f32 {
    let it = call!(g.player_interact, fn() -> P);
    if !alive(it) {
        return 2.0;
    }
    field_of(it, "interactDistance").map_or(2.0, |o| rd(it, o))
}

/// Whether E would grab the float now: the screen centre meets one of its solid colliders within
/// reach, with nothing solid in between (the roof hides all but what sticks out).
unsafe fn crosshair_on_float(g: &G, cam: P, float: P, player_layer: i32, reach: f32) -> bool {
    let cam_tr = call!(g.comp_transform, fn(P) -> P, cam);
    let ray = Ray { origin: call!(g.tr_get_pos, fn(P) -> V3, cam_tr), dir: call!(g.tr_forward, fn(P) -> V3, cam_tr) };
    let mut dist = f32::MAX;
    for col in il2cpp::array(rd(float, g.off_float_colliders)) {
        let mut hit = Hit::default();
        let solid = alive(col) && call!(g.col_enabled, fn(P) -> bool, col) && !call!(g.col_trigger, fn(P) -> bool, col);
        if solid && call!(g.col_raycast, fn(P, Ray, *mut Hit, f32) -> bool, col, ray, &mut hit, reach) {
            dist = dist.min(hit.distance);
        }
    }
    if dist > reach {
        return false;
    }
    let float_layer = call!(g.go_layer, fn(P) -> i32, call!(g.comp_gameobject, fn(P) -> P, float));
    let mask = collision_mask(g, player_layer, player_layer) & !(1 << float_layer);
    let mut hit = Hit::default();
    !call!(g.sphere_cast, fn(V3, f32, V3, *mut Hit, f32, i32, i32) -> bool, ray.origin, 0.01, ray.dir, &mut hit, dist, mask, IGNORE_TRIGGERS)
}

/// One step of the looping test per frame: load the next (spot, turn), let the float settle, turn
/// the head until the preview is green, drop with the game's own CmdDrop body, then once through
/// the roof look at the float and grab it like E would.
unsafe fn run_repeat(g: &G, s: &mut State, ctrl: P, tr: P, carried: Option<P>, identity: P, player_layer: Option<i32>) {
    let Some(mut r) = s.repeat.take() else { return };
    let Some(a) = s.slots.get(r.from).cloned() else { return };
    let b = r.to.and_then(|i| s.slots.get(i).cloned());
    let spots = spot_count(&a, b.as_ref());
    if r.greens.len() != spots {
        r.greens = vec![None; spots];
    }
    let float = a.float as P;
    let float_pos = || transform_pos(g, float).unwrap_or_default();
    let turned = |r: &Repeat, k: usize| Quat::yaw(turn_deg(k)).mul(r.aim.0);
    let green = || s.drop.is_some_and(|d| d.stays_up());
    let side = || s.drop.map(|d| drop_side(s.pos, s.yaw, d.pos));
    if matches!(r.phase, Phase::Settle(_) | Phase::Scan { .. } | Phase::Hold { .. }) && (carried != Some(float) || !alive(identity)) {
        s.repeat_error = Some("Arrete : la bouee n'est pas revenue dans tes mains.");
        return;
    }
    let phase = r.phase;
    r.phase = match phase {
        Phase::Load => {
            match r.greens.iter().position(|g| g.is_none()) {
                Some(i) => (r.spot, r.turn) = (i, None),
                None => match next_cell(&cell_stats(&s.trials, spots), &green_cells(&r.greens, None)) {
                    Some(c) => (r.spot, r.turn) = (c / TURNS, Some(c % TURNS)),
                    None => {
                        s.repeat_error = Some("Arrete : aucun angle ne donne de cercle vert.");
                        return;
                    },
                },
            }
            let saved = spot_at(&a, b.as_ref(), r.spot, spots);
            r.aim = (saved.rot, saved.look);
            (r.grab, r.side) = (None, None);
            s.actions.push(Action::LoadAt(saved));
            Phase::Settle(0)
        },
        Phase::Settle(f) if f < SETTLE_FRAMES => {
            // Undo any mouse movement: every try starts from the spot's aim.
            call!(g.tr_set_rot, fn(P, Quat), tr, r.aim.0);
            wr(ctrl, g.off_look, r.aim.1);
            Phase::Settle(f + 1)
        },
        Phase::Settle(_) if r.turn.is_none() => {
            r.scan.clear();
            Phase::Scan { k: 0, frame: 0 }
        },
        Phase::Settle(_) => Phase::Hold { frame: 0, green: 0 },
        Phase::Scan { k, frame } if k < TURNS => {
            call!(g.tr_set_rot, fn(P, Quat), tr, turned(&r, k));
            if frame + 1 < SEEK_HOLD {
                Phase::Scan { k, frame: frame + 1 }
            } else {
                // You found the float tends to fall when the green circle is on your right.
                r.scan.push(green() && !(s.skip_right && side() == Some(Side::Right)));
                Phase::Scan { k: k + 1, frame: 0 }
            }
        },
        Phase::Scan { .. } => {
            r.greens[r.spot] = Some(std::mem::take(&mut r.scan));
            match next_cell(&cell_stats(&s.trials, spots), &green_cells(&r.greens, Some(r.spot))) {
                Some(c) => {
                    r.turn = Some(c % TURNS);
                    Phase::Hold { frame: 0, green: 0 }
                },
                None => {
                    s.trials.push(Trial { spot: r.spot, turn: None, outcome: Outcome::NeverGreen, float_pos: float_pos(), grab: None, side: None });
                    Phase::Load
                },
            }
        },
        Phase::Hold { frame, green: n } => {
            let k = r.turn.unwrap_or(TURNS / 2);
            call!(g.tr_set_rot, fn(P, Quat), tr, turned(&r, k));
            let n = if frame > 0 && green() { n + 1 } else { 0 };
            if n >= GREEN_FRAMES {
                r.side = side();
                call!(g.carry_drop, fn(P, P), float, identity);
                Phase::Watch { t: 0.0, start_y: s.pos.y }
            } else if frame >= HOLD_FRAMES {
                // Green during the sweep but not now: a failed try of this line up.
                s.trials.push(Trial { spot: r.spot, turn: Some(k), outcome: Outcome::NeverGreen, float_pos: float_pos(), grab: None, side: None });
                Phase::Load
            } else {
                Phase::Hold { frame: frame + 1, green: n }
            }
        },
        Phase::Watch { t, start_y } if t < WATCH_SECS => {
            // Through the roof and empty-handed: look at the float's top and press E once it's under the crosshair.
            if carried.is_none() && s.pos.y - start_y > ROOF_RISE {
                if let (Some(target), Some(layer)) = (s.float_top_pos.or_else(|| transform_pos(g, float)), player_layer) {
                    let target = V3 { y: target.y - 0.03, ..target };
                    let d = target.sub(s.pos);
                    call!(g.tr_set_rot, fn(P, Quat), tr, Quat::yaw(d.x.atan2(d.z).to_degrees()));
                    call!(g.align_pitch, fn(P, V3, f32), ctrl, target, 1.0e4);
                    let cam = rd::<P>(ctrl, g.off_camera);
                    if alive(cam) && crosshair_on_float(g, cam, float, layer, interact_reach(g)) {
                        call!(g.carry_pickup, fn(P, P), float, identity);
                        r.grab = Some(target);
                    }
                }
            }
            Phase::Watch { t: t + call!(g.delta_time, fn() -> f32), start_y }
        },
        Phase::Watch { start_y, .. } => {
            let outcome = classify(s.pos.y - start_y, carried == Some(float));
            let grab = r.grab.filter(|_| outcome == Outcome::Success);
            s.trials.push(Trial { spot: r.spot, turn: r.turn, outcome, float_pos: float_pos(), grab, side: r.side });
            Phase::Load
        },
    };
    s.repeat = Some(r);
}

/// Runs every frame on Unity's main thread, right after the local player's Update.
unsafe fn tick(g: &G, ctrl: P) {
    let mut guard = lock();
    let s = &mut *guard;
    let stats = local_stats();
    let tr = call!(g.comp_transform, fn(P) -> P, ctrl);
    if stats.is_null() || tr.is_null() {
        return;
    }
    s.last_tick = Some(Instant::now());

    // Max values aren't exposed by the game: use the highest seen since this player spawned.
    if s.stats_obj != stats as usize {
        s.stats_obj = stats as usize;
        s.max = [0.0; 4];
    }
    for i in 0..4 {
        s.stats[i] = rd::<f32>(stats, g.off_stats[i]);
        s.max[i] = s.max[i].max(s.stats[i]);
    }

    let god = GOD.load(Relaxed);
    if god {
        wr(stats, g.off_stats[0], s.max[0]);
    }
    if god || s.applied_god {
        wr(stats, g.off_invuln, god);
    }
    s.applied_god = god;
    if s.inf_stamina {
        wr(stats, g.off_stats[1], s.max[1]);
    }
    if s.inf_oxygen {
        wr(stats, g.off_stats[2], s.max[2]);
    }
    if s.no_anxiety {
        wr(stats, g.off_stats[3], 0.0f32);
    }

    // Movement multipliers scale the shared PlayerVariablesInfo asset from its original values.
    let vars = call!(g.player_vars, fn() -> P);
    if alive(vars) {
        if s.vars_orig.as_ref().map(|(p, _)| *p) != Some(vars as usize) {
            s.vars_orig = Some((vars as usize, g.off_vars.iter().map(|o| rd::<f32>(vars, *o)).collect()));
            s.applied_mults = (1.0, 1.0);
        }
        if (s.speed_mult, s.jump_mult) != s.applied_mults {
            let orig = &s.vars_orig.as_ref().unwrap().1;
            for (i, (off, v)) in g.off_vars.iter().zip(orig).enumerate() {
                wr(vars, *off, v * if i == g.off_vars.len() - 1 { s.jump_mult } else { s.speed_mult });
            }
            s.applied_mults = (s.speed_mult, s.jump_mult);
        }
    }

    let grounded = rd::<P>(ctrl, g.off_grounded);
    if alive(grounded) {
        s.fall_time = rd(grounded, g.off_falling_time);
        s.ground_below = !rd::<bool>(grounded, g.off_falling_inf);
    }

    let pos = call!(g.tr_get_pos, fn(P) -> V3, tr);
    let noclip = NOCLIP.load(Relaxed);
    if noclip {
        if !s.applied_noclip {
            s.fly_pos = pos;
        }
        let cam = rd::<P>(ctrl, g.off_camera);
        if alive(cam) && !s.menu_open && crate::game_focused() {
            let cam_tr = call!(g.comp_transform, fn(P) -> P, cam);
            let (fwd, right) = (call!(g.tr_forward, fn(P) -> V3, cam_tr), call!(g.tr_right, fn(P) -> V3, cam_tr));
            let axis = |p: bool, n: bool| p as i32 as f32 - n as i32 as f32;
            let speed = s.fly_speed * if key(VK_LSHIFT.0) { 3.0 } else { 1.0 } * call!(g.delta_time, fn() -> f32);
            s.fly_pos = s
                .fly_pos
                .add_scaled(fwd, axis(key_at(SC_W), key_at(SC_S)) * speed)
                .add_scaled(right, axis(key_at(SC_D), key_at(SC_A)) * speed)
                .add_scaled(V3 { x: 0.0, y: 1.0, z: 0.0 }, axis(key(VK_SPACE.0), key(VK_LCONTROL.0)) * speed);
        }
        // The CharacterController moved us during Update; override it and push the change to physics.
        call!(g.tr_set_pos, fn(P, V3), tr, s.fly_pos);
        call!(g.sync_transforms, fn());
        wr(ctrl, g.off_gravity, 0.0f32);
    }
    s.applied_noclip = noclip;
    s.pos = if noclip { s.fly_pos } else { pos };
    let fwd = call!(g.tr_forward, fn(P) -> V3, tr);
    s.yaw = fwd.x.atan2(fwd.z).to_degrees();
    s.game_time = call!(g.game_time, fn() -> f32);

    if s.time_scale != s.applied_time_scale {
        call!(g.time_set_scale, fn(f32), s.time_scale);
        s.applied_time_scale = s.time_scale;
    }

    // The game re-locks the cursor on its own, so keep it free every frame while the menu is open.
    if s.menu_open {
        if s.cursor_saved.is_none() {
            s.cursor_saved =
                Some((call!(g.cursor_lock_get, fn() -> i32), call!(g.cursor_visible_get, fn() -> bool)));
        }
        call!(g.cursor_lock_set, fn(i32), 0);
        call!(g.cursor_visible_set, fn(bool), true);
    } else if let Some((lock_state, visible)) = s.cursor_saved.take() {
        call!(g.cursor_lock_set, fn(i32), lock_state);
        call!(g.cursor_visible_set, fn(bool), visible);
    }

    let manager = call!(g.entities_singleton, fn() -> P);
    let monsters: Vec<P> =
        if alive(manager) { il2cpp::list(rd(manager, g.off_spawned)).into_iter().filter(|&m| alive(m)).collect() } else { Vec::new() };
    if s.freeze_entities || s.applied_freeze {
        for &m in &monsters {
            call!(g.go_set_active, fn(P, bool), call!(g.comp_gameobject, fn(P) -> P, m), !s.freeze_entities);
        }
    }
    s.applied_freeze = s.freeze_entities;
    s.entities.clear();
    if s.menu_open {
        for &m in &monsters {
            let go = call!(g.comp_gameobject, fn(P) -> P, m);
            s.entities.push(EntityInfo {
                id: m as usize,
                name: name_of(g, go),
                pos: transform_pos(g, m).unwrap_or_default(),
                active: call!(g.go_active, fn(P) -> bool, go),
            });
        }
    }

    // The Poolrooms float this player carries, if any: its carrier is our network id.
    let identity = call!(g.get_component, fn(P, P) -> P, ctrl, g.t_identity);
    let my_id = if alive(identity) { call!(g.net_id, fn(P) -> u32, identity) } else { 0 };
    if s.last_float_scan.is_none_or(|t| t.elapsed() > Duration::from_secs(1)) {
        s.floats = find(g, g.t_carry).into_iter().map(|f| f as usize).collect();
        scan_float_colliders(g, s, s.pos);
        s.last_float_scan = Some(Instant::now());
    }
    let carried =
        s.floats.iter().map(|&f| f as P).find(|&f| my_id != 0 && alive(f) && rd::<u32>(f, g.off_carrier) == my_id);
    let cc = rd::<P>(ctrl, g.off_character);
    let player_layer = alive(cc).then(|| call!(g.go_layer, fn(P) -> i32, call!(g.comp_gameobject, fn(P) -> P, cc)));
    s.drop = match (carried, player_layer) {
        (Some(f), Some(layer)) if s.drop_preview => Some(drop_preview(g, f, tr, layer)),
        _ => None,
    };
    track_dropped_float(g, s, carried, cc, player_layer);
    run_repeat(g, s, ctrl, tr, carried, identity, player_layer);

    let cam = rd::<P>(ctrl, g.off_camera);
    s.camera = cam as usize;
    if s.show_colliders || s.show_triggers {
        let here = s.pos;
        if s.last_scan.is_none_or(|t| t.elapsed() > Duration::from_secs(1)) {
            scan_colliders(g, s, ctrl, here);
            s.last_scan = Some(Instant::now());
        }
    } else if s.last_scan.is_some() {
        s.colliders.clear();
        s.last_scan = None;
    }
    // Normally drawn from begin_camera; this is the fallback if HDRP never goes through it.
    if !s.camera_hook_drew && alive(cam) {
        draw(g, s, cam);
    }

    if s.items.is_empty() {
        s.items = il2cpp::array(call!(g.item_names, fn() -> P)).into_iter().map(|p| il2cpp::string(p)).collect();
        s.items.sort();
    }

    let inv = call!(g.player_inventory, fn() -> P);
    for action in std::mem::take(&mut s.actions) {
        let action = match action {
            Action::Load(i) => match s.slots.get(i) {
                Some(saved) => {
                    s.selected = i;
                    Action::LoadAt(saved.clone())
                },
                None => continue,
            },
            other => other,
        };
        match action {
            Action::Save => {
                let rot = call!(g.tr_get_rot, fn(P) -> Quat, tr);
                let items = if alive(inv) { inventory(g, inv, &s.items) } else { Vec::new() };
                let float = carried.map_or(0, |f| f as usize);
                let (look, boost) = (rd(ctrl, g.off_look), (rd(stats, g.off_boost), rd(stats, g.off_boost_timer)));
                s.slots.push(Saved { pos: s.pos, rot, stats: s.stats, items, float, look, boost });
                s.selected = s.slots.len() - 1;
            },
            Action::Load(_) => unreachable!("turned into LoadAt above"),
            Action::LoadAt(saved) => {
                s.fly_pos = saved.pos;
                teleport(g, ctrl, saved.pos, saved.rot, false);
                wr(ctrl, g.off_look, saved.look);
                for (off, v) in g.off_stats.iter().zip(saved.stats) {
                    wr(stats, *off, v);
                }
                wr(stats, g.off_boost, saved.boost.0);
                wr(stats, g.off_boost_timer, saved.boost.1);
                // Hand the float back (the server-side body of CmdPickup) unless we still carry it.
                let float = saved.float as P;
                if !float.is_null() && alive(float) && alive(identity) && carried != Some(float) {
                    call!(g.carry_pickup, fn(P, P), float, identity);
                }
                if s.restore_inventory && alive(inv) {
                    let have = inventory(g, inv, &s.items);
                    for name in &s.items {
                        let count = |list: &[(String, i32)]| list.iter().find(|(n, _)| n == name).map_or(0, |(_, c)| *c);
                        let diff = count(&saved.items) - count(&have);
                        let item = il2cpp::new_string(name);
                        if diff < 0 {
                            call!(g.remove_item, fn(P, P, i32) -> P, inv, item, -diff);
                        }
                        for _ in 0..diff {
                            call!(g.add_item, fn(P, P), inv, item);
                        }
                    }
                }
            },
            Action::Give(name) => {
                if alive(inv) {
                    call!(g.add_item, fn(P, P), inv, il2cpp::new_string(&name));
                }
            },
            Action::Goto(p) => {
                s.fly_pos = p;
                let rot = call!(g.tr_get_rot, fn(P) -> Quat, tr);
                teleport(g, ctrl, p, rot, !noclip);
            },
            Action::ToggleEntity(id) => {
                if let Some(&m) = monsters.iter().find(|&&m| m as usize == id) {
                    let go = call!(g.comp_gameobject, fn(P) -> P, m);
                    call!(g.go_set_active, fn(P, bool), go, !call!(g.go_active, fn(P) -> bool, go));
                }
            },
            Action::ScanPoints => s.points = scan_points(g),
            Action::UnlockLevelsForever => s.unlock_msg = Some(unlock_levels_forever(g)),
        }
    }
}

/// The distinct unlock stats of (level, stat) pairs; stat 0 (COMPLETE_TIME) unlocks nothing.
fn unlock_stats(levels: &[(i32, i32)]) -> Vec<i32> {
    let mut stats: Vec<i32> = levels.iter().map(|&(_, stat)| stat).filter(|&stat| stat != 0).collect();
    stats.sort_unstable();
    stats.dedup();
    stats
}

/// Does for every level what reaching it does in v0.7.0: its "REACH_*" stat set to 1 (saved
/// with the player's Steam stats, so it stays) and the level opened right away.
unsafe fn unlock_levels_forever(g: &G) -> String {
    let Some(u) = &g.level_unlock else { return "Pas disponible dans cette version du jeu.".into() };
    let mgr = call!(u.instance, fn() -> P);
    if mgr.is_null() {
        return "Progression du jeu introuvable : reessaie en pleine partie.".into();
    }
    let pairs: Vec<(i32, i32)> = g
        .level_names
        .iter()
        .filter(|(v, _)| *v > 0 && *v < 9999) // not PLAYERS_LOBBY, UNKNOWN or EVENT_LEVEL
        .filter_map(|&(v, _)| {
            let info = call!(u.get_info, fn(i32) -> P, v);
            (!info.is_null()).then(|| (v, rd::<i32>(info, u.off_stat)))
        })
        .collect();
    let stats = unlock_stats(&pairs);
    for &stat in &stats {
        // Static, like ZoneReachStatTrigger calls it when a level is reached.
        call!(u.set_stat, fn(i32, i32), stat, 1);
        call!(u.mark, fn(P, i32), mgr, stat);
    }
    format!("{} niveaux debloques pour de bon.", stats.len())
}

/// Saves a Volume's weight the first time it is overridden, restores it when `off` goes false.
unsafe fn override_weight(g: &G, saved: &mut BTreeMap<usize, f32>, vol: P, off: bool) {
    if off && alive(vol) {
        saved.entry(vol as usize).or_insert_with(|| rd(vol, g.off_weight));
        wr(vol, g.off_weight, 0.0f32);
    } else if !off {
        for (v, w) in std::mem::take(saved) {
            if alive(v as P) {
                wr(v as P, g.off_weight, w);
            }
        }
    }
}

/// Camera effects and brightness, right after PlayerCamera.Update.
unsafe fn visuals(g: &G, cam: P) {
    let mut guard = lock();
    let s = &mut *guard;

    // paniniProjection, vhsEffect, lensDistortion in off_fx: they warp the image away from the hitbox lines.
    const WARPING_FX: [usize; 3] = [3, 8, 9];
    let hitboxes = s.show_colliders || s.show_triggers;
    for (i, &off) in g.off_fx.iter().enumerate() {
        let c = rd::<P>(cam, off);
        if c.is_null() {
            continue;
        }
        if s.fx_off || (hitboxes && WARPING_FX.contains(&i)) {
            s.fx_saved.entry(c as usize).or_insert_with(|| rd(c, g.off_active));
            wr(c, g.off_active, false);
        } else if let Some(active) = s.fx_saved.remove(&(c as usize)) {
            wr(c, g.off_active, active);
        }
    }

    let gm = call!(g.gm_singleton, fn() -> P);
    let vhs = if alive(gm) { rd::<P>(gm, g.off_gm_vhs) } else { std::ptr::null_mut() };
    let mgr = call!(g.bm_singleton, fn() -> P);
    let info = if alive(mgr) { call!(g.bm_level_info, fn(P) -> P, mgr) } else { std::ptr::null_mut() };
    let level_post = if info.is_null() { std::ptr::null_mut() } else { rd::<P>(info, g.off_level_post) };
    override_weight(g, &mut s.vhs_saved, vhs, s.fx_off);
    override_weight(g, &mut s.post_saved, level_post, s.level_post_off);

    // Brightness: offset ColorAdjustments.postExposure. The camera may rewrite it, so anything
    // other than our last written value becomes the new base.
    let adj = rd::<P>(cam, g.off_color_adj);
    let param = if adj.is_null() { std::ptr::null_mut() } else { rd::<P>(adj, g.off_post_exposure) };
    if param.is_null() {
        return;
    }
    let Ok(off_value) = field_of(param, "m_Value") else { return };
    if s.brightness != 0.0 || s.exposure.is_some() {
        let cur = rd::<f32>(param, off_value);
        let (base, override_orig) = match s.exposure {
            Some((p, base, written, ov)) if p == param as usize && cur == written => (base, ov),
            Some((p, _, _, ov)) if p == param as usize => (cur, ov),
            _ => (cur, rd(param, g.off_override)),
        };
        if s.brightness != 0.0 {
            wr(param, off_value, base + s.brightness);
            wr(param, g.off_override, true);
            s.exposure = Some((param as usize, base, base + s.brightness, override_orig));
        } else {
            wr(param, off_value, base);
            wr(param, g.off_override, override_orig);
            s.exposure = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_and_bounds() {
        // Identity view, simple perspective (w = -z): a point straight ahead lands at the centre.
        let mut proj = [0.0; 16];
        proj[0] = 1.0;
        proj[5] = 1.0;
        proj[11] = -1.0; // row 3, col 2
        let vp = Mat4(proj).mul(&Mat4([1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.]));
        assert_eq!(vp.project(V3 { x: 0.0, y: 0.0, z: -5.0 }), Some([0.0, 0.0]));
        assert_eq!(vp.project(V3 { x: 5.0, y: -5.0, z: -5.0 }), Some([1.0, -1.0]));
        assert_eq!(vp.project(V3 { x: 0.0, y: 0.0, z: 5.0 }), None); // behind
        let b = Bounds { center: V3::default(), extents: V3 { x: 1.0, y: 1.0, z: 1.0 } };
        assert_eq!(aabb_dist(&b, V3 { x: 0.5, y: 0.0, z: 0.0 }), 0.0);
        assert_eq!(aabb_dist(&b, V3 { x: 4.0, y: 0.0, z: 0.0 }), 3.0);
    }

    #[test]
    fn clipping_keeps_the_part_in_front() {
        // w is the depth: a segment from 1 m ahead to 1 m behind is cut at w = NEAR_W.
        let seg = clip_front(&[[0.0, 0.0, 0.0, 1.0], [2.0, 0.0, 0.0, -1.0]]);
        assert_eq!(seg.len(), 3); // a, cut, cut: callers take the first two
        assert_eq!(seg[0], [0.0, 0.0]);
        assert!((seg[1][0] - 0.95 / NEAR_W).abs() < 1e-3);
        assert!(clip_front(&[[0.0, 0.0, 0.0, -1.0], [1.0, 0.0, 0.0, -2.0]]).is_empty());
        let tri = clip_front(&[[0.0, 0.0, 0.0, 1.0], [1.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, -1.0]]);
        assert_eq!(tri.len(), 4); // one corner behind: the triangle becomes a quad
    }

    #[test]
    fn mesh_outline_drops_flat_diagonals() {
        // A cube the way Unity stores it: 6 faces of 4 unshared vertices, 2 triangles each.
        let b = box_shape(V3::default(), V3 { x: 1.0, y: 2.0, z: 3.0 });
        let mut verts = Vec::new();
        let mut idx = Vec::new();
        for f in b.faces.chunks(2) {
            let base = verts.len() as i32;
            verts.extend([f[0][0], f[0][1], f[0][2], f[1][2]]);
            idx.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        assert_eq!(outline(&verts, &idx).len(), 12);
        assert_eq!(outline(&verts[..4], &idx[..6]).len(), 4); // one flat quad: its 4 sides
        idx.push(99); // a broken index must not panic
        idx.extend([0, 1, 99]);
        assert_eq!(outline(&verts, &idx).len(), 12);
        assert_eq!(b.edges.len(), 12);
        assert_eq!(b.faces.len(), 12);
    }

    #[test]
    fn sampling_lands_on_the_surface() {
        // Unit sphere hit test standing in for Collider.Raycast.
        let sphere = |from: V3, to: V3| {
            let d = to.sub(from);
            let (a, b, c) = (d.dot(d), 2.0 * from.dot(d), from.dot(from) - 1.0);
            let disc = b * b - 4.0 * a * c;
            let t = (-b - disc.max(0.0).sqrt()) / (2.0 * a);
            (disc >= 0.0 && (0.0..=1.0).contains(&t)).then(|| from.add_scaled(d, t))
        };
        let b = Bounds { center: V3::default(), extents: V3 { x: 1.0, y: 1.0, z: 1.0 } };
        let s = sample_shape(b, sphere);
        assert!(!s.edges.is_empty() && s.edges.len() < 2000, "{} edges", s.edges.len());
        for (a, b) in &s.edges {
            assert!((a.dist(V3::default()) - 1.0).abs() < 1e-3 && (b.dist(V3::default()) - 1.0).abs() < 1e-3);
        }
        assert!(sample_shape(b, |_, _| None).edges.is_empty());
    }

    #[test]
    fn drop_verdict() {
        let at = |y: f32, land: Option<f32>, inside| DropInfo {
            pos: V3 { x: 0.0, y, z: 0.0 },
            radius: 0.4,
            land: land.map(|l| V3 { x: 0.0, y: l, z: 0.0 }),
            inside,
        };
        let (green, orange, red) = ([0.2, 1.0, 0.3, 1.0], [1.0, 0.6, 0.1, 1.0], [1.0, 0.2, 0.2, 1.0]);
        assert_eq!(at(10.0, Some(9.7), false).color(), green); // lands on the roof
        assert_eq!(at(10.0, Some(8.0), false).color(), orange);
        assert_eq!(at(10.0, Some(2.0), false).color(), red); // back into the room
        assert_eq!(at(10.0, None, false).color(), red);
        assert_eq!(at(10.0, Some(9.7), true).color(), orange); // spawns inside the slab
        assert_eq!(at(10.0, Some(2.0), false).label(), "retombe de 8.0 m");
    }

    #[test]
    fn invisible_walls_by_name() {
        for n in ["InvisibleWall (3)", "BoundsCollider", "LiftBoundsCollider", "BoundsCollider_cacheCollider", "LimitExitCollider"] {
            assert!(invisible_name(n), "{n}");
        }
        for n in ["Concrete Pillar_24_cacheCollider", "Metal_Barrier2_cacheCollider", "Inflatable_Ring_Unicorn"] {
            assert!(!invisible_name(n), "{n}");
        }
    }

    #[test]
    fn dropped_float_tracking() {
        assert_eq!(just_dropped(0, 5), None); // picked up
        assert_eq!(just_dropped(5, 5), None); // still carried
        assert_eq!(just_dropped(5, 0), Some(5));
        assert_eq!(just_dropped(5, 7), Some(5)); // swapped for another one
        let mut trail = Vec::new();
        extend_trail(&mut trail, V3::default());
        extend_trail(&mut trail, V3 { x: 0.005, y: 0.0, z: 0.0 }); // hasn't really moved
        extend_trail(&mut trail, V3 { x: 0.5, y: 0.0, z: 0.0 });
        assert_eq!(trail.len(), 2);
        assert_eq!(top_label(0.081), "+8 cm");
        assert_eq!(top_label(-0.2), "-20 cm");
        assert_eq!(top_label(-0.004), "+0 cm");
    }

    #[test]
    fn repeat_trials_judged() {
        assert_eq!(classify(2.5, true), Outcome::Success);
        assert_eq!(classify(2.5, false), Outcome::HornHidden);
        assert_eq!(classify(0.1, true), Outcome::NotThrough);
        assert_eq!([turn_deg(0), turn_deg(TURNS / 2), turn_deg(TURNS - 1)], [-60.0, 0.0, 60.0]);
        // A quarter turn to the right takes forward (+Z) to +X, as Unity does.
        let q = Quat::yaw(90.0).mul(Quat::yaw(0.0)).0;
        let (x, z) = (2.0 * (q[0] * q[2] + q[3] * q[1]), 1.0 - 2.0 * (q[0] * q[0] + q[1] * q[1]));
        assert!((x - 1.0).abs() < 1e-5 && z.abs() < 1e-5);
        assert!((yaw_of(Quat::yaw(30.0).mul(Quat::yaw(10.0))) - 40.0).abs() < 1e-3);
        // Facing +X (yaw 90): +X is in front, -Z on the right, -X behind, +Z on the left.
        let o = V3::default();
        let side = |x, z| drop_side(o, 90.0, V3 { x, y: 0.0, z });
        assert_eq!([side(1.0, 0.0), side(0.0, -1.0), side(-1.0, 0.0), side(0.0, 1.0)], Side::ALL);
        let at = |x: f32| V3 { x, y: 0.0, z: 0.0 };
        assert_eq!(spread(&[at(1.0), at(1.0)]), 0.0);
        assert!((spread(&[at(0.0), at(0.3), at(-0.2)]) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn spots_between_two_slots() {
        let slot = |x: f32| Saved {
            pos: V3 { x, y: 0.0, z: 0.0 },
            rot: Quat([0.0, 0.0, 0.0, 1.0]),
            stats: [0.0; 4],
            items: Vec::new(),
            float: 1,
            look: V3 { x: 0.0, y: x * 10.0, z: 0.0 },
            boost: (true, 0.0),
        };
        let (a, b) = (slot(0.0), slot(0.5));
        assert_eq!(spot_count(&a, None), 1);
        assert_eq!(spot_count(&a, Some(&b)), 11); // every 5 cm, both ends included
        assert_eq!(spot_count(&a, Some(&slot(100.0))), MAX_SPOTS);
        let mid = spot_at(&a, Some(&b), 5, 11);
        assert!((mid.pos.x - 0.25).abs() < 1e-6 && (mid.look.y - 2.5).abs() < 1e-5);
        assert_eq!(spot_at(&a, Some(&b), 10, 11).pos, b.pos);
        let trial = |spot, turn, outcome| Trial { spot, turn, outcome, float_pos: V3::default(), grab: None, side: None };
        let trials = [
            trial(0, Some(3), Outcome::Success),
            trial(1, Some(20), Outcome::Success),
            trial(1, Some(20), Outcome::NotThrough),
            trial(1, None, Outcome::NeverGreen),
        ];
        assert_eq!(spot_stats(&trials, 2), vec![(1, 1), (3, 1)]);
        let cells = cell_stats(&trials, 2);
        assert_eq!((cells[3], cells[TURNS + 20], cells[TURNS + 21]), ((1, 1), (2, 1), (0, 0)));
    }

    #[test]
    fn best_line_up_forgives_errors() {
        assert_eq!(best_cell(&[(0, 0)], 1), None);
        assert_eq!(best_cell(&[(1, 1), (0, 0), (0, 0), (10, 9)], 1), Some(3)); // 9/10 beats a lucky 1/1
        // One column: a 4/4 spot between two failing ones loses to a wide area working 3 times out of 4.
        let stats = [(4, 0), (4, 4), (4, 0), (4, 3), (4, 3), (4, 3)];
        assert_eq!(zone_stats(&stats, 1, 0), (8, 4));
        assert_eq!(best_cell(&stats, 1), Some(4));
        // Two spots of three turns: the zone spans both spots and the next turns.
        let stats = [(1, 1), (2, 0), (0, 0), (3, 3), (1, 0), (0, 0)];
        assert_eq!(zone_stats(&stats, 3, 0), (7, 4));
        assert_eq!(zone_stats(&stats, 3, 5), (3, 0));
    }

    #[test]
    fn test_focuses_on_what_works() {
        let mut greens = vec![None; 2];
        greens[1] = Some((0..TURNS).map(|k| k == 5 || k == 19 || k == 30).collect());
        let cells = green_cells(&greens, None);
        assert_eq!(cells, [TURNS + 5, TURNS + 19, TURNS + 30]);
        assert_eq!(green_cells(&greens, Some(0)), []);
        let mut stats = vec![(0, 0); 2 * TURNS];
        assert_eq!(next_cell(&stats, &cells), Some(TURNS + 19)); // untried, closest to the slot's aim
        for &c in &cells {
            stats[c] = (10, 1);
        }
        stats[TURNS + 30] = (10, 9);
        assert_eq!(next_cell(&stats, &cells), Some(TURNS + 30));
        stats[TURNS + 5] = (1, 0);
        assert_eq!(next_cell(&stats, &cells), Some(TURNS + 5)); // barely tried: worth another look
        assert_eq!(next_cell(&stats, &[]), None);
    }

    #[test]
    fn unlock_stats_skip_none_and_duplicates() {
        // (level, its unlock stat): 0 is COMPLETE_TIME, never an unlock stat (the Lobby is always open).
        assert_eq!(unlock_stats(&[(1, 0), (12, 56), (11, 60), (2, 1), (13, 56)]), vec![1, 56, 60]);
    }

    #[test]
    fn capsule_spans_its_height() {
        let s = capsule_shape(V3::default(), 0.5, 3.0, 1);
        let top = s.edges.iter().flat_map(|e| [e.0.y, e.1.y]).fold(f32::MIN, f32::max);
        let bottom = s.edges.iter().flat_map(|e| [e.0.y, e.1.y]).fold(f32::MAX, f32::min);
        assert!((top - 1.5).abs() < 1e-4 && (bottom + 1.5).abs() < 1e-4);
    }
}
