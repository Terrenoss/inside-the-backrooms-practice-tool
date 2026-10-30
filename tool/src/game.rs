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

    fn translation(t: V3) -> Mat4 {
        Mat4::from_basis(AXES[0], AXES[1], AXES[2], t)
    }

    /// Axes x, y, z (rotation and scale) as columns, then the translation t.
    fn from_basis(x: V3, y: V3, z: V3, t: V3) -> Mat4 {
        Mat4([x.x, x.y, x.z, 0.0, y.x, y.y, y.z, 0.0, z.x, z.y, z.z, 0.0, t.x, t.y, t.z, 1.0])
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
}

/// Where the carried float would end up if dropped now, in world space.
#[derive(Clone, Copy)]
pub struct DropInfo {
    pos: V3,
    radius: f32,
    land: Option<V3>,
    inside: bool,
    /// Where it appears from the player's body (see side_of).
    side: &'static str,
    /// How far its top will stick out of the roof above (negative: stays under it), if there is one.
    top: Option<f32>,
    /// It would appear inside the player and push them out of it: the drop-and-jump boost.
    on_player: bool,
}

impl DropInfo {
    pub fn label(&self) -> String {
        let boost = if self.on_player { "dans toi (te pousse) | " } else { "" };
        let verdict = match (self.inside, self.land.map(|l| self.pos.y - l.y)) {
            (true, _) => "dans le decor : sera ejectee".into(),
            (_, None) => "rien dessous : tombe dans le vide".into(),
            (_, Some(fall)) if fall > 1.0 => format!("retombe de {fall:.1} m"),
            (_, Some(fall)) => format!("reste en haut ({fall:.1} m)"),
        };
        let roof = match self.top {
            Some(t) if t > 0.0 => format!(" | depasse du toit de {:.0} cm", t * 100.0),
            Some(t) => format!(" | sous le toit, il manque {:.0} cm", -t * 100.0),
            None => String::new(),
        };
        format!("{} : {boost}{verdict}{roof}", self.side)
    }

    /// For the float's own shape: green when it will stick out of the roof (grab it back from there).
    pub fn roof_color(&self) -> [f32; 4] {
        match self.top {
            Some(t) if t > 0.0 => [0.2, 1.0, 0.3, 1.0],
            Some(_) => [1.0, 0.2, 0.2, 1.0],
            None => [1.0, 1.0, 1.0, 0.8],
        }
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

/// How the player moved during the last item inspection (the inspect glitch).
#[derive(Clone, Copy)]
pub struct Push {
    from: V3,
    to: V3,
    /// Horizontal facing when the inspection opened.
    fwd: V3,
    /// Something solid between where the player was and where they ended up.
    crossed: bool,
    radius: f32,
    height: f32,
    at: Instant,
}

impl Push {
    pub fn recent(&self) -> bool {
        self.at.elapsed() < PUSH_SHOWN
    }

    fn moved(&self) -> bool {
        self.from.dist(self.to) >= 0.05
    }

    pub fn label(&self) -> String {
        if !self.moved() {
            return "pas pousse".into();
        }
        let d = self.to.sub(self.from);
        let right = V3 { x: self.fwd.z, y: 0.0, z: -self.fwd.x };
        let text = format!(
            "pousse de {:.2} m (avant {:+.2}, droite {:+.2}, haut {:+.2})",
            self.from.dist(self.to),
            d.dot(self.fwd),
            d.dot(right),
            d.y
        );
        if self.crossed { text + " : A TRAVERSE" } else { text }
    }

    pub fn color(&self) -> [f32; 4] {
        match (self.moved(), self.crossed) {
            (false, _) => [0.7, 0.7, 0.7, 1.0],
            (true, true) => [0.2, 1.0, 0.3, 1.0],
            (true, false) => [1.0, 0.6, 0.1, 1.0],
        }
    }
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
    /// Ends the level (exit zone, end door, end interactable): drawn from any distance.
    Exit,
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
    Give(String),
    Goto(V3),
    ToggleEntity(usize),
    ScanPoints,
}

// Read by hooks too, so kept outside the mutex.
pub static GOD: AtomicBool = AtomicBool::new(false);
pub static NOCLIP: AtomicBool = AtomicBool::new(false);
pub static INF_ITEMS: AtomicBool = AtomicBool::new(false);
pub static NO_FALL_TP: AtomicBool = AtomicBool::new(false);

#[derive(Default)]
pub struct State {
    pub menu_open: bool,
    pub free_cam: bool,
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
    pub inspect_preview: bool,
    /// Name of the last inspected item, whose pooled copy the preview draws ("" before any).
    pub inspect_item: String,
    /// Whether inspected items (INSPECT_LAYER) are solid for the player.
    pub inspect_solid: Option<bool>,
    pub push: Option<Push>,
    pub show_colliders: bool,
    pub show_visible_walls: bool,
    pub show_triggers: bool,
    pub show_exits: bool,
    pub collider_radius: f32,
    pub actions: Vec<Action>,
    pub slots: Vec<Saved>,
    pub selected: usize,
    // Filled by the game thread for the overlay.
    pub last_tick: Option<Instant>,
    pub pos: V3,
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
    pub exit_count: usize,
    // Game-thread bookkeeping.
    stats_obj: usize,
    max: [f32; 4],
    fly_pos: V3,
    applied_god: bool,
    applied_noclip: bool,
    applied_free_cam: bool,
    /// Free camera position, where the body is held meanwhile, and the camera's real position
    /// while a frame renders from the free one.
    cam_pos: V3,
    cam_yaw: f32,
    cam_pitch: f32,
    /// Mouse look taken from the player (InputLookDir) for the free camera.
    look_input: [f32; 2],
    pin_pos: V3,
    cam_saved: Option<(V3, Quat)>,
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
    /// The carried float's colliders in its own space (read while carried), how high it reaches
    /// above its pivot, and which float that was.
    float_shapes: Vec<(Mat4, Shape)>,
    float_top: f32,
    float_of_shapes: usize,
    /// Where those shapes would be if dropped now.
    drop_model: Option<Mat4>,
    /// Float carried last frame, and the one lying dropped since when (drawn to grab it back),
    /// where it appeared, and where it ended up.
    carried_last: usize,
    dropped: Option<(usize, Instant)>,
    drop_spawn: Option<V3>,
    pub landed: Option<Landed>,
    /// Its path over the first seconds after the drop.
    drop_trail: Vec<V3>,
    /// Copied when an inspection opens: the game frees the item and its colliders on level change.
    /// (collider space to pivot space, shape)
    inspect_shapes: Vec<(Mat4, Shape)>,
    /// Local player controller: a new one means a new level, whose unload freed the cached objects.
    controller: usize,
    /// Player capsule (center, radius, height) last frame, and when the inspection opened.
    capsule: Option<(V3, f32, f32)>,
    inspect_start: Option<(V3, V3)>,
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
        inspect_preview: true,
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

// What can end a level (every caller of LevelEndings.Perform): (class, end id field, required item
// field, label). BackroomsExitZone fires for any collider under the local player, not just its body.
// EndPartyLevelDoor has no item field: its Interact checks the inventory for "FunDoorKey", written in
// its code.
const LEVEL_ENDS: [(&str, &str, &str, &str); 3] = [
    ("BackroomsExitZone", "endGameID", "", "FIN DU NIVEAU"),
    ("EndPartyLevelDoor", "endGameId", "", "FIN : porte [E] (il faut FunDoorKey)"),
    ("EndInteractable", "endGameID", "requiredItem", "FIN : interaction [E]"),
];

/// One kind of LEVEL_ENDS found in this game version.
struct LevelEnd {
    ty: P,
    off_id: usize,
    off_item: Option<usize>,
    label: &'static str,
    /// Entered rather than used (BackroomsExitZone).
    zone: bool,
}

const ELEVATORS: [&str; 4] = ["MainElevator", "Elevator", "HotelElevator", "LiftElevator"];

// Inspect glitch: ItemInspectionUI.ShowInspectable puts the inspected item's copy this far in front of
// the player camera, its pivot at world rotation identity; FinalizeInspectable gives it this layer and
// AddInspectionCollider a solid collider (mesh, or its bounds as a box). The player gets pushed out
// of it, sometimes through bars or curtains.
const INSPECT_DISTANCE: f32 = 0.1;
const INSPECT_LAYER: i32 = 30;
const PUSH_SHOWN: Duration = Duration::from_secs(15);
/// How long a dropped float stays drawn (until picked up again).
const DROPPED_SHOWN: Duration = Duration::from_secs(60);
/// A dropped float's resting place is measured from LANDED_AFTER to LANDED_BY after the drop.
const LANDED_AFTER: Duration = Duration::from_millis(500);
const LANDED_BY: Duration = Duration::from_secs(3);

/// LevelBoundaries (level start / end zones), None before 0.8.0.
struct Boundaries {
    ty: P,
    level: M,
    off_start: usize,
    off_end: usize,
}

/// ItemInspectionUI, None if this game version renamed it.
struct Inspection {
    singleton: M,
    is_inspecting: M,
    off_current: usize,
    off_pivot: usize,
}

struct G {
    player_controller: M,
    player_stats: M,
    player_inventory: M,
    player_camera: M,
    player_vars: M,
    game_time: M,
    teleport: M,
    /// Teleport takes holdForGround (0.8.0+), not before.
    teleport_hold: bool,
    comp_transform: M,
    comp_gameobject: M,
    get_component: M,
    get_components: M,
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
    go_transform: M,
    tr_child_count: M,
    tr_get_child: M,
    cc_center: M,
    cc_radius: M,
    cc_height: M,
    insp: Option<Inspection>,
    renderer_enabled: M,
    go_layer: M,
    ignore_layers: M,
    ignore_pair: M,
    item_names: M,
    add_item: M,
    remove_item: M,
    item_count: M,
    carry_drop_pos: M,
    carry_pickup: M,
    net_id: M,
    tr_scale: M,
    sphere_cast: M,
    check_sphere: M,
    off_carrier: usize,
    off_floater_radius: usize,
    off_floater_colliders: usize,
    t_carry: P,
    t_identity: P,
    entities_singleton: M,
    gm_singleton: M,
    bm_singleton: M,
    bm_level_info: M,
    boundaries: Option<Boundaries>,
    off_camera: usize,
    off_character: usize,
    off_gravity: usize,
    off_grounded: usize,
    off_falling_time: usize,
    off_falling_inf: usize,
    off_stats: [usize; 4],
    off_invuln: usize,
    off_spawned: usize,
    off_vars: Vec<usize>, // speeds then jumpForce (last)
    off_look_dir: usize,
    off_look_speed: usize,
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
    off_swap_level: usize,
    off_tp_dest: usize,
    t_collider: P,
    t_renderer: P,
    k_box: P,
    k_sphere: P,
    k_capsule: P,
    k_mesh: P,
    k_character: P,
    t_swap: P,
    t_tp: P,
    t_elevators: Vec<P>,
    specials: Vec<(P, &'static str)>,
    level_ends: Vec<LevelEnd>,
    level_names: Vec<(i32, String)>,
    // Hook targets.
    pc_update: M,
    cam_update: M,
    begin_camera: M,
    begin_context: M,
    end_context: M,
    take_damage: M,
    kill_player: M,
    monster_kill: M,
    infinite_falling: M,
    wear_item: M,
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
    let (swap, tp) = (k("BackroomsLevelSwapTrigger")?, k("ITBTeleportTrigger")?);
    let (teleport, teleport_hold) = match method(bpc, "Teleport", 3) {
        Ok(m) => (m, true),
        Err(_) => (method(bpc, "Teleport", 2)?, false),
    };
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
        teleport,
        teleport_hold,
        comp_transform: method(comp, "get_transform", 0)?,
        comp_gameobject: method(comp, "get_gameObject", 0)?,
        get_component: method_sig(comp, "GetComponent", &["System.Type"])?,
        get_components: method_sig(comp, "GetComponents", &["System.Type"])?,
        obj_name: method(object, "get_name", 0)?,
        find_objects: method_sig(object, "FindObjectsByType", &["System.Type", "UnityEngine.FindObjectsSortMode"])?,
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
        off_floater_colliders: f(carry, "m_Colliders")?,
        t_carry: ty(carry),
        t_identity: ty(identity),
        get_component_in_children: method_sig(comp, "GetComponentInChildren", &["System.Type", "System.Boolean"])?,
        go_transform: method(go, "get_transform", 0)?,
        tr_child_count: method(tr, "get_childCount", 0)?,
        tr_get_child: method(tr, "GetChild", 1)?,
        cc_center: method(character, "get_center", 0)?,
        cc_radius: method(character, "get_radius", 0)?,
        cc_height: method(character, "get_height", 0)?,
        insp: k("ItemInspectionUI").ok().and_then(|c| {
            Some(Inspection {
                singleton: method(c, "get_Singleton", 0).ok()?,
                is_inspecting: method(c, "get_IsInspecting", 0).ok()?,
                off_current: f(c, "m_CurrentInspectable").ok()?,
                off_pivot: f(c, "inspectionPivot").ok()?,
            })
        }),
        renderer_enabled: method(renderer, "get_enabled", 0)?,
        go_layer: method(go, "get_layer", 0)?,
        ignore_layers: method(physics, "GetIgnoreLayerCollision", 2)?,
        ignore_pair: method(physics, "GetIgnoreCollision", 2)?,
        item_names: method(items, "GetItemsNames", 0)?,
        add_item: method(inv, "ServerAddItem", 1)?,
        remove_item: method(inv, "ServerRemoveItem", 2)?,
        item_count: method(inv, "GetItemCount", 1)?,
        entities_singleton: method(ents, "get_Singleton", 0)?,
        gm_singleton: method(gm, "get_singleton", 0)?,
        bm_singleton: method(bm, "get_Singleton", 0)?,
        bm_level_info: method(bm, "get_CurrentLevelInfo", 0)?,
        boundaries: k("LevelBoundaries").ok().and_then(|c| {
            Some(Boundaries {
                ty: ty(c),
                level: method(c, "get_Level", 0).ok()?,
                off_start: f(c, "startZone").ok()?,
                off_end: f(c, "endZones").ok()?,
            })
        }),
        off_camera: f(bpc, "playerCamera")?,
        off_character: f(pc, "Controller")?,
        off_gravity: f(pc, "m_GravityForce")?,
        off_grounded: f(bpc, "groundedChecker")?,
        off_falling_time: f(grounded, "<FallingTime>k__BackingField")?,
        off_falling_inf: f(grounded, "<FallingToInfinte>k__BackingField")?,
        off_stats: [f(stats, "health")?, f(stats, "m_Stamina")?, f(stats, "m_Oxygen")?, f(stats, "anxiety")?],
        off_invuln: f(stats, "m_HasInvulnerability")?,
        off_spawned: f(ents, "SpawnedEntities")?,
        off_look_dir: f(pc, "InputLookDir")?,
        off_look_speed: f(vars, "lookSpeed")?,
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
        off_swap_level: f(swap, "newLevel")?,
        off_tp_dest: f(tp, "teleportPoint")?,
        t_collider: ty(collider),
        t_renderer: ty(renderer),
        k_box: boxc,
        k_sphere: sphere,
        k_capsule: capsule,
        k_mesh: meshc,
        k_character: character,
        t_swap: ty(swap),
        t_tp: ty(tp),
        t_elevators: ELEVATORS.iter().filter_map(|n| k(n).ok()).map(|c| ty(c)).collect(),
        specials: SPECIAL_TRIGGERS.iter().filter_map(|(ns, n, l)| class(ac, ns, n).ok().map(|c| (ty(c), *l))).collect(),
        level_ends: LEVEL_ENDS
            .iter()
            .filter_map(|&(n, id, item, label)| {
                let c = k(n).ok()?;
                let off_item = if item.is_empty() { None } else { Some(f(c, item).ok()?) };
                Some(LevelEnd { ty: ty(c), off_id: f(c, id).ok()?, off_item, label, zone: n == "BackroomsExitZone" })
            })
            .collect(),
        level_names: il2cpp::enum_values(k("BackroomsLevel")?),
        pc_update: method(pc, "Update", 0)?,
        cam_update: method(cam, "Update", 0)?,
        begin_camera: method(render_pipeline, "BeginCameraRendering", 2)?,
        begin_context: method(render_pipeline, "BeginContextRendering", 2)?,
        end_context: method(render_pipeline, "EndContextRendering", 2)?,
        take_damage: method(stats, "ServerTakeDamage", 1)?,
        kill_player: method(stats, "CmdKillPlayer", 1)?,
        // Not BasePlayerController's: empty, so its code is a stub shared by ~1300 other methods.
        monster_kill: method(pc, "PerformMonsterKill", 4)?,
        infinite_falling: method(bpc, "CheckForInfinteFalling", 0)?,
        wear_item: method(inv, "ServerWearItem", 2)?,
    })
}

fn g() -> &'static G {
    G.get().unwrap()
}

// Trampolines to the original functions.
static O_UPDATE: AtomicUsize = AtomicUsize::new(0);
static O_CAM_UPDATE: AtomicUsize = AtomicUsize::new(0);
static O_BEGIN_CAMERA: AtomicUsize = AtomicUsize::new(0);
static O_BEGIN_CONTEXT: AtomicUsize = AtomicUsize::new(0);
static O_END_CONTEXT: AtomicUsize = AtomicUsize::new(0);
static O_DAMAGE: AtomicUsize = AtomicUsize::new(0);
static O_KILL: AtomicUsize = AtomicUsize::new(0);
static O_MONSTER_KILL: AtomicUsize = AtomicUsize::new(0);
static O_FALLING: AtomicUsize = AtomicUsize::new(0);
static O_WEAR: AtomicUsize = AtomicUsize::new(0);

/// Called once from the init thread, after hudhook initialized MinHook.
pub unsafe fn install() -> Result<(), String> {
    let thread = (il2cpp::api().thread_attach)((il2cpp::api().domain_get)());
    let r = resolve();
    (il2cpp::api().thread_detach)(thread);
    _ = G.set(r.map_err(|e| format!("introuvable dans le jeu : {e}"))?);
    let g = g();
    crate::log(&format!(
        "{} niveaux, {}/{} types de triggers, {}/{} types de fins, {}/{} ascenseurs, inspection {}",
        g.level_names.len(),
        g.specials.len(),
        SPECIAL_TRIGGERS.len(),
        g.level_ends.len(),
        LEVEL_ENDS.len(),
        g.t_elevators.len(),
        ELEVATORS.len(),
        if g.insp.is_some() { "ok" } else { "introuvable" }
    ));
    let hooks: [(M, usize, &AtomicUsize); 10] = [
        (g.pc_update, pc_update as usize, &O_UPDATE),
        (g.cam_update, cam_update as usize, &O_CAM_UPDATE),
        (g.begin_camera, begin_camera as usize, &O_BEGIN_CAMERA),
        (g.begin_context, begin_context as usize, &O_BEGIN_CONTEXT),
        (g.end_context, end_context as usize, &O_END_CONTEXT),
        (g.take_damage, take_damage as usize, &O_DAMAGE),
        (g.kill_player, kill_player as usize, &O_KILL),
        (g.monster_kill, monster_kill as usize, &O_MONSTER_KILL),
        (g.infinite_falling, infinite_falling as usize, &O_FALLING),
        (g.wear_item, wear_item as usize, &O_WEAR),
    ];
    for (target, detour, orig) in hooks {
        let h = MhHook::new(target.0 as _, detour as _).map_err(|e| format!("hook: {e:?}"))?;
        orig.store(h.trampoline() as usize, Relaxed);
        h.queue_enable().map_err(|e| format!("hook: {e:?}"))?;
    }
    MH_ApplyQueued().ok().map_err(|e| format!("hook: {e:?}"))
}

/// `hold`: wait for the ground to stream in before letting the player fall (0.8.0+ only).
unsafe fn teleport(g: &G, ctrl: P, pos: V3, rot: Quat, hold: bool) {
    if g.teleport_hold {
        call!(g.teleport, fn(P, V3, Quat, bool), ctrl, pos, rot, hold)
    } else {
        call!(g.teleport, fn(P, V3, Quat), ctrl, pos, rot)
    }
}

/// Unity's Quaternion.Euler(pitch, yaw, 0), in degrees: yaw around Y, then pitch (down > 0).
fn yaw_pitch_rotation(yaw: f32, pitch: f32) -> Quat {
    let (sy, cy) = (yaw.to_radians() / 2.0).sin_cos();
    let (sp, cp) = (pitch.to_radians() / 2.0).sin_cos();
    Quat([cy * sp, sy * cp, -sy * sp, cy * cp])
}

/// Forward and right of that rotation.
fn yaw_pitch_axes(yaw: f32, pitch: f32) -> (V3, V3) {
    let (sy, cy) = yaw.to_radians().sin_cos();
    let (sp, cp) = pitch.to_radians().sin_cos();
    (V3 { x: sy * cp, y: -sp, z: cy * cp }, V3 { x: cy, y: 0.0, z: -sy })
}

/// Yaw and pitch (degrees) looking along `f`.
fn yaw_pitch_of(f: V3) -> (f32, f32) {
    (f.x.atan2(f.z).to_degrees(), (-f.y).clamp(-1.0, 1.0).asin().to_degrees())
}

/// This frame's ZQSD / Space / Ctrl move along a view (Shift x3), for noclip and the free camera.
unsafe fn fly_step(g: &G, s: &State, fwd: V3, right: V3) -> V3 {
    if s.menu_open || !crate::game_focused() {
        return V3::default();
    }
    let axis = |p: bool, n: bool| p as i32 as f32 - n as i32 as f32;
    let speed = s.fly_speed * if key(VK_LSHIFT.0) { 3.0 } else { 1.0 } * call!(g.delta_time, fn() -> f32);
    V3::default()
        .add_scaled(fwd, axis(key_at(SC_W), key_at(SC_S)) * speed)
        .add_scaled(right, axis(key_at(SC_D), key_at(SC_A)) * speed)
        .add_scaled(V3 { x: 0.0, y: 1.0, z: 0.0 }, axis(key(VK_SPACE.0), key(VK_LCONTROL.0)) * speed)
}

unsafe fn local_controller() -> P {
    call!(g().player_controller, fn() -> P)
}

unsafe fn local_stats() -> P {
    call!(g().player_stats, fn() -> P)
}

unsafe extern "C-unwind" fn pc_update(this: P, mi: usize) {
    let orig: unsafe extern "C-unwind" fn(P, usize) = std::mem::transmute(O_UPDATE.load(Relaxed));
    let ours = !this.is_null() && this == local_controller();
    if ours {
        // Free camera: the mouse turns it, not the player. OnLookInput (input callback, before
        // Update) stores the look delta that UpdateLookRotation applies during this Update.
        let mut s = lock();
        if s.free_cam {
            let d: [f32; 2] = rd(this, g().off_look_dir);
            s.look_input = [s.look_input[0] + d[0], s.look_input[1] + d[1]];
            wr(this, g().off_look_dir, [0.0f32; 2]);
        }
    }
    orig(this, mi);
    if ours {
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

/// The player's camera among the ones HDRP is about to render (live objects, unlike s.camera).
unsafe fn rendered_player_camera(s: &State, cameras: P) -> Option<P> {
    il2cpp::list(cameras).into_iter().find(|&c| c as usize == s.camera && alive(c))
}

// Free camera: the player's camera is moved for the frame's render only (end_context puts it back),
// so the game, and the float held at the view (PlayerGear.floaterAnchorPoint), keep the real one.
// HDRP works out each camera's view (PrepareAndCullCamera) after BeginContextRendering but before
// BeginCameraRendering: this is the last point where moving it still changes the image.
unsafe extern "C-unwind" fn begin_context(context: usize, cameras: P, mi: usize) {
    {
        let mut guard = lock();
        let s = &mut *guard;
        if let Some(cam) = rendered_player_camera(s, cameras).filter(|_| s.applied_free_cam) {
            let tr = call!(g().comp_transform, fn(P) -> P, cam);
            if s.cam_saved.is_none() {
                s.cam_saved = Some((call!(g().tr_get_pos, fn(P) -> V3, tr), call!(g().tr_get_rot, fn(P) -> Quat, tr)));
            }
            call!(g().tr_set_pos, fn(P, V3), tr, s.cam_pos);
            call!(g().tr_set_rot, fn(P, Quat), tr, yaw_pitch_rotation(s.cam_yaw, s.cam_pitch));
        }
    }
    let orig: unsafe extern "C-unwind" fn(usize, P, usize) = std::mem::transmute(O_BEGIN_CONTEXT.load(Relaxed));
    orig(context, cameras, mi);
}

unsafe extern "C-unwind" fn end_context(context: usize, cameras: P, mi: usize) {
    let orig: unsafe extern "C-unwind" fn(usize, P, usize) = std::mem::transmute(O_END_CONTEXT.load(Relaxed));
    orig(context, cameras, mi);
    let mut guard = lock();
    let s = &mut *guard;
    if let Some(cam) = rendered_player_camera(s, cameras) {
        if let Some((p, r)) = s.cam_saved.take() {
            let tr = call!(g().comp_transform, fn(P) -> P, cam);
            call!(g().tr_set_pos, fn(P, V3), tr, p);
            call!(g().tr_set_rot, fn(P, Quat), tr, r);
        }
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
    for lb in &g.boundaries {
        for b in find(g, lb.ty) {
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
    }
    for end in &g.level_ends {
        for c in find(g, end.ty) {
            add(end_label(end, c), transform_pos(g, c));
        }
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

unsafe fn end_label(end: &LevelEnd, comp: P) -> String {
    let mut label = format!("{} #{}", end.label, rd::<i32>(comp, end.off_id));
    let item = end.off_item.map_or(String::new(), |off| il2cpp::string(rd(comp, off)));
    if !item.is_empty() {
        label += &format!(" (il faut {item})");
    }
    label
}

/// Everything that currently ends the level: (collider, or the component itself when its object
/// has none, is it a collider, label).
unsafe fn level_ends(g: &G) -> Vec<(P, bool, String)> {
    let mut out = Vec::new();
    for end in &g.level_ends {
        for c in find(g, end.ty) {
            let label = end_label(end, c);
            let cols = il2cpp::array(call!(g.get_components, fn(P, P) -> P, c, g.t_collider));
            if cols.is_empty() {
                out.push((c, false, label));
                continue;
            }
            for col in cols {
                // A solid one only works through OnCollisionEnter (useCollision).
                let solid = end.zone && !call!(g.col_trigger, fn(P) -> bool, col);
                out.push((col, true, if solid { format!("{label} - au contact") } else { label.clone() }));
            }
        }
    }
    out
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
        !call!(g.ignore_layers, fn(i32, i32) -> bool, own, layer) && !call!(g.ignore_pair, fn(P, P) -> bool, cc, col)
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
        sphere_shape(v3(g.sphere_center), f(g.sphere_radius))
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

/// Sphere as three great circles.
fn sphere_shape(c: V3, r: f32) -> Shape {
    let mut edges = Vec::new();
    for i in 0..3 {
        arc(&mut edges, c, AXES[i], AXES[(i + 1) % 3], r, 0.0, TAU);
    }
    Shape { edges, ..Default::default() }
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

/// Size of the marker drawn for a level end with no collider of its own.
const MARKER_RADIUS: f32 = 0.5;

fn cache(infos: &mut BTreeMap<(usize, usize), Arc<Info>>, key: (usize, usize), info: Info) -> Arc<Info> {
    let info = Arc::new(info);
    infos.insert(key, info.clone());
    info
}

unsafe fn scan_colliders(g: &G, s: &mut State, ctrl: P, pos: V3) {
    let cc = rd::<P>(ctrl, g.off_character);
    let player = alive(cc).then(|| (cc, call!(g.go_layer, fn(P) -> i32, call!(g.comp_gameobject, fn(P) -> P, cc))));
    let radius = s.collider_radius * 1.5;
    // ~1000 raycasts per sampled shape: spread them over scans instead of one long hitch.
    let mut budget = 6;
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    // Level ends first and at any distance: the nearby scan below leaves their colliders alone.
    let ends = if s.show_exits { level_ends(g) } else { Vec::new() };
    for (obj, collider, label) in &ends {
        let (obj, collider) = (*obj, *collider);
        if collider && !call!(g.col_enabled, fn(P) -> bool, obj) {
            continue;
        }
        let key = (obj as usize, rd::<usize>(obj, 0x10));
        seen.insert(key);
        let bounds = if collider {
            call!(g.col_bounds, fn(P) -> Bounds, obj)
        } else {
            let Some(center) = transform_pos(g, obj) else { continue };
            Bounds { center, extents: V3 { x: MARKER_RADIUS, y: MARKER_RADIUS, z: MARKER_RADIUS } }
        };
        let info = match s.infos.get(&key).filter(|i| i.kind == Kind::Exit) {
            Some(info) => info.clone(),
            None => {
                let drawn = if collider {
                    shape(g, obj, (il2cpp::api().object_get_class)(obj), &mut budget)
                } else {
                    Some(sphere_shape(V3::default(), MARKER_RADIUS))
                };
                let Some(drawn) = drawn else { continue }; // sampling budget spent: next scan
                cache(&mut s.infos, key, Info { kind: Kind::Exit, label: label.clone(), shape: drawn })
            },
        };
        if !info.shape.edges.is_empty() {
            out.push(Col { obj: obj as usize, info, bounds });
        }
    }
    s.exit_count = out.len();
    for c in if s.show_colliders || s.show_triggers { find(g, g.t_collider) } else { Vec::new() } {
        let k = (il2cpp::api().object_get_class)(c);
        if k == g.k_character || !call!(g.col_enabled, fn(P) -> bool, c) || ends.iter().any(|e| e.0 == c) {
            continue;
        }
        let key = (c as usize, rd::<usize>(c, 0x10));
        seen.insert(key);
        let bounds = call!(g.col_bounds, fn(P) -> Bounds, c);
        if aabb_dist(&bounds, pos) > radius {
            continue;
        }
        let info = match s.infos.get(&key).filter(|i| i.kind != Kind::Exit) {
            Some(info) => info.clone(),
            None => match describe(g, c, k, player, &mut budget) {
                Some(info) => cache(&mut s.infos, key, info),
                None => continue, // sampling budget spent: next scan
            },
        };
        if info.kind != Kind::Ignored && !info.shape.edges.is_empty() {
            out.push(Col { obj: c as usize, info, bounds });
        }
    }
    s.infos.retain(|k, _| seen.contains(k));
    s.collider_count = out.len() - s.exit_count;
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
    let hitboxes = s.show_colliders || s.show_triggers || s.show_exits;
    let inspect = s.inspect_preview && !s.inspect_shapes.is_empty() || s.push.is_some_and(|p| p.recent());
    // From the free camera, the player's own body usually isn't drawn: show where it is.
    let body = s.capsule.filter(|_| s.applied_free_cam);
    // s.floats is rescanned every second and emptied on level change: a float still in it is safe.
    let dropped = s
        .dropped
        .filter(|(f, t)| s.drop_preview && t.elapsed() < DROPPED_SHOWN && s.floats.contains(f))
        .map(|(f, _)| f as P)
        .filter(|&f| alive(f));
    if !hitboxes && s.drop.is_none() && !inspect && body.is_none() && dropped.is_none() {
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
    if let Some(f) = dropped {
        draw_dropped(g, s, &vp, f);
    }
    if inspect {
        draw_inspect(g, s, &vp);
    }
    if let Some((c, r, h)) = body {
        for (a, b) in capsule_shape(c, r, h, 1).edges {
            world_line(&mut s.lines, &vp, a, b, [1.0, 1.0, 1.0, 0.8]);
        }
    }
}

/// A world-space segment as an overlay line, cut to what's in front of the camera.
fn world_line(out: &mut Vec<Line>, vp: &Mat4, a: V3, b: V3, color: [f32; 4]) {
    if let [a, b, ..] = clip_front(&[vp.point(a), vp.point(b)])[..] {
        out.push(Line { a, b, color });
    }
}

/// The float as it would appear if dropped now (the game's sphere and its real shape), and a line
/// down to where it would land.
fn draw_drop(s: &mut State, vp: &Mat4, d: DropInfo) {
    let color = d.color();
    let mut edges = sphere_shape(d.pos, d.radius).edges;
    if let Some(land) = d.land {
        edges.push((d.pos, land));
        arc(&mut edges, land, AXES[0], AXES[2], d.radius, 0.0, TAU);
    }
    for (a, b) in edges {
        world_line(&mut s.lines, vp, a, b, color);
    }
    if let Some(m) = s.drop_model {
        let mut lines = Vec::new();
        for (in_float, shape) in &s.float_shapes {
            let m = m.mul(in_float);
            for &(a, b) in &shape.edges {
                world_line(&mut lines, vp, m.apply(a), m.apply(b), d.roof_color());
            }
        }
        s.lines.extend(lines);
    }
    if let Some(p) = vp.project(d.pos) {
        s.labels.push((p, format!("DROP : {}", d.label()), color));
    }
}

/// The float lying where it was dropped, through walls and roof: what to aim at to grab it back.
unsafe fn draw_dropped(g: &G, s: &mut State, vp: &Mat4, float: P) {
    let world = call!(g.tr_matrix, fn(P) -> Mat4, call!(g.comp_transform, fn(P) -> P, float));
    let color = [1.0, 0.3, 1.0, 1.0];
    let mut lines = Vec::new();
    // Where it went after the drop, from where it appeared.
    for w in s.drop_spawn.iter().chain(&s.drop_trail).collect::<Vec<_>>().windows(2) {
        world_line(&mut lines, vp, *w[0], *w[1], [1.0, 0.3, 1.0, 0.6]);
    }
    for (in_float, shape) in &s.float_shapes {
        let m = world.mul(in_float);
        for &(a, b) in &shape.edges {
            world_line(&mut lines, vp, m.apply(a), m.apply(b), color);
        }
    }
    s.lines.extend(lines);
    let top = world.apply(V3::default()).add_scaled(AXES[1], s.float_top);
    if let Some(p) = vp.project(top) {
        let below = s.pos.y - top.y;
        s.labels.push((p, format!("BOUEE : vise-la + E (sommet {below:.1} m sous tes pieds)"), color));
    }
}

/// QueryTriggerInteraction.Ignore
const IGNORE_TRIGGERS: i32 = 1;

/// Layer mask of what `layer` collides with, minus `skip`.
unsafe fn collision_mask(g: &G, layer: i32, skip: &[i32]) -> i32 {
    (0..32)
        .filter(|l| !skip.contains(l) && !call!(g.ignore_layers, fn(i32, i32) -> bool, layer, *l))
        .fold(0i32, |m, l| m | 1 << l)
}

/// Which way from the player's body an offset points. GetSafeDropPosition puts the float at feet
/// height + dropHeight, in the first clear direction among front, right, back, left of the body
/// (the camera's pitch plays no part), or right above if none is.
fn side_of(d: V3, fwd: V3, right: V3) -> &'static str {
    let (f, r) = (d.dot(fwd), d.dot(right));
    if f.abs().max(r.abs()) < 0.05 {
        "au-dessus de toi"
    } else if f.abs() >= r.abs() {
        if f > 0.0 { "devant" } else { "derriere" }
    } else if r > 0.0 {
        "a droite"
    } else {
        "a gauche"
    }
}

/// A mesh collider's own triangles (a convex one's hull hugs them), or its bounds if unreadable:
/// no raycast sampling, which needs the collider enabled.
unsafe fn plain_mesh_shape(g: &G, col: P) -> Option<Shape> {
    let mesh = call!(g.mc_mesh, fn(P) -> P, col);
    if !alive(mesh) {
        return None;
    }
    if call!(g.mesh_readable, fn(P) -> bool, mesh) {
        let idx: Vec<i32> = il2cpp::values(call!(g.mesh_triangles, fn(P) -> P, mesh));
        if idx.len() <= 3 * MAX_MESH_TRIS {
            let verts: Vec<V3> = il2cpp::values(call!(g.mesh_vertices, fn(P) -> P, mesh));
            return Some(Shape { edges: outline(&verts, &idx), ..Default::default() });
        }
    }
    let b = call!(g.mesh_bounds, fn(P) -> Bounds, mesh);
    Some(Shape { approx: true, ..box_shape(b.center, b.extents) })
}

/// The float's colliders in its own space and how high it reaches above its pivot once upright.
/// Read while carried, with its colliders disabled.
unsafe fn float_shapes(g: &G, float: P) -> (Vec<(Mat4, Shape)>, f32) {
    let tr = call!(g.comp_transform, fn(P) -> P, float);
    let (to_float, scale) = (call!(g.tr_inverse, fn(P) -> Mat4, tr), call!(g.tr_scale, fn(P) -> V3, tr));
    let shapes: Vec<(Mat4, Shape)> = il2cpp::array(rd(float, g.off_floater_colliders))
        .into_iter()
        .filter(|&c| alive(c))
        .filter_map(|c| {
            let k = (il2cpp::api().object_get_class)(c);
            let drawn = if k == g.k_mesh { plain_mesh_shape(g, c)? } else { shape(g, c, k, &mut 0)? };
            let m = call!(g.tr_matrix, fn(P) -> Mat4, call!(g.comp_transform, fn(P) -> P, c));
            Some((to_float.mul(&m), drawn))
        })
        .collect();
    let top = shapes
        .iter()
        .flat_map(|(m, shape)| shape.edges.iter().flat_map(move |&(a, b)| [m.apply(a).y, m.apply(b).y]))
        .fold(f32::NEG_INFINITY, f32::max);
    (shapes, if top.is_finite() { top * scale.y.abs() } else { 0.0 })
}

/// Distance to the first solid thing for `mask` along a ray (a thin sphere cast).
unsafe fn ray(g: &G, from: V3, dir: V3, max: f32, mask: i32) -> Option<f32> {
    let mut hit = Hit::default();
    call!(g.sphere_cast, fn(V3, f32, V3, *mut Hit, f32, i32, i32) -> bool, from, 0.02, dir, &mut hit, max, mask, IGNORE_TRIGGERS)
        .then_some(hit.distance)
}

/// Where the carried float would go if dropped now: the game's own GetSafeDropPosition (what
/// CmdDrop uses), a sphere cast down to where it would land, and how far its top would stick out
/// of the roof above (UserCode_CmdDrop turns it to the player's yaw). Also gives where its shapes
/// (`float_top` high) would be drawn.
unsafe fn drop_preview(g: &G, float: P, player_tr: P, player_layer: i32, float_top: f32) -> (DropInfo, Mat4) {
    let pos = call!(g.carry_drop_pos, fn(P, P) -> V3, float, player_tr);
    let feet = call!(g.tr_get_pos, fn(P) -> V3, player_tr);
    let (fwd, right) = (call!(g.tr_forward, fn(P) -> V3, player_tr), call!(g.tr_right, fn(P) -> V3, player_tr));
    let side = side_of(pos.sub(feet), fwd, right);
    let scale = call!(g.tr_scale, fn(P) -> V3, call!(g.comp_transform, fn(P) -> P, float));
    let radius = rd::<f32>(float, g.off_floater_radius) * scale.x.abs();
    // What the float collides with, minus players: it would land on you instead.
    let layer = call!(g.go_layer, fn(P) -> i32, call!(g.comp_gameobject, fn(P) -> P, float));
    let mask = collision_mask(g, layer, &[player_layer]);
    let inside = call!(g.check_sphere, fn(V3, f32, i32, i32) -> bool, pos, radius * 0.9, mask, IGNORE_TRIGGERS);
    let (up, down) = (AXES[1], AXES[1].scale(-1.0));
    let mut hit = Hit::default();
    let land = call!(g.sphere_cast, fn(V3, f32, V3, *mut Hit, f32, i32, i32) -> bool, pos, radius, down, &mut hit, 500.0, mask, IGNORE_TRIGGERS)
        .then(|| pos.add_scaled(down, hit.distance));
    let rest = if inside { pos } else { land.unwrap_or(pos) };
    let top = roof_margin(g, feet.add_scaled(up, 0.5), rest, float_top, collision_mask(g, layer, &[player_layer, layer]));
    let flat = |v: V3| {
        let v = V3 { y: 0.0, ..v };
        v.scale(1.0 / v.dot(v).sqrt().max(1e-6))
    };
    let model = Mat4::from_basis(flat(right).scale(scale.x), up.scale(scale.y), flat(fwd).scale(scale.z), rest);
    (DropInfo { pos, radius, land, inside, side, top, on_player: false }, model)
}

/// How far a float resting at `base`, its top `top` m above it, sticks out of the roof over it
/// (negative: under it), if there's one: the ceiling found going up from `from`, then the roof's
/// upper face over `base`, found from above. `mask` leaves players and floats out.
unsafe fn roof_margin(g: &G, from: V3, base: V3, top: f32, mask: i32) -> Option<f32> {
    let (up, down) = (AXES[1], AXES[1].scale(-1.0));
    ray(g, from, up, 10.0, mask).map(|d| {
        let above = V3 { y: from.y + d + 3.0, ..base };
        let roof = ray(g, above, down, 3.0, mask).map_or(from.y + d, |d| above.y - d);
        base.y + top - roof
    })
}

/// Where a dropped float ended up once settled.
#[derive(Clone, Copy)]
pub struct Landed {
    /// Its top against the roof above (negative: under it), if there's one.
    top: Option<f32>,
    /// How far below where it appeared.
    fell: f32,
    at: Instant,
}

impl Landed {
    pub fn recent(&self) -> bool {
        self.at.elapsed() < PUSH_SHOWN
    }

    pub fn label(&self) -> String {
        match self.top {
            _ if self.fell > 1.0 => format!("tombee de {:.1} m", self.fell),
            Some(t) if t > 0.0 => format!("contre le toit, depasse de {:.0} cm : vise-la d'en haut + E", t * 100.0),
            Some(t) => format!("restee en haut, {:.0} cm sous le toit", -t * 100.0),
            None => "restee en haut, pas de toit au-dessus".into(),
        }
    }

    pub fn color(&self) -> [f32; 4] {
        match self.top {
            _ if self.fell > 1.0 => [1.0, 0.2, 0.2, 1.0],
            Some(t) if t > 0.0 => [0.2, 1.0, 0.3, 1.0],
            _ => [1.0, 0.6, 0.1, 1.0],
        }
    }
}

/// Does a sphere overlap the player's upright capsule (center, radius, height)?
fn sphere_in_capsule(c: V3, r: f32, (cc, cr, ch): (V3, f32, f32)) -> bool {
    let half = (ch / 2.0 - cr).max(0.0);
    let y = c.y.clamp(cc.y - half, cc.y + half);
    c.dist(V3 { y, ..cc }) < r + cr
}

/// The player's CharacterController in world space: (center, radius, height). Always upright.
unsafe fn capsule(g: &G, cc: P) -> (V3, f32, f32) {
    let tr = call!(g.comp_transform, fn(P) -> P, cc);
    let center = call!(g.tr_matrix, fn(P) -> Mat4, tr).apply(call!(g.cc_center, fn(P) -> V3, cc));
    (center, call!(g.cc_radius, fn(P) -> f32, cc), call!(g.cc_height, fn(P) -> f32, cc))
}

unsafe fn collect_colliders(g: &G, tr: P, depth: u32, out: &mut Vec<P>) {
    out.extend(il2cpp::array(call!(g.get_components, fn(P, P) -> P, tr, g.t_collider)));
    if depth < 8 {
        for i in 0..call!(g.tr_child_count, fn(P) -> i32, tr) {
            collect_colliders(g, call!(g.tr_get_child, fn(P, i32) -> P, tr, i), depth + 1, out);
        }
    }
}

/// Shapes of the inspected copy's colliders and where they sit in the pivot (AddInspectionCollider
/// only makes readable meshes and boxes: no raycast sampling needed).
unsafe fn item_shapes(g: &G, item: P, pivot: P) -> Vec<(Mat4, Shape)> {
    let to_pivot = call!(g.tr_inverse, fn(P) -> Mat4, pivot);
    let mut cols = Vec::new();
    collect_colliders(g, call!(g.go_transform, fn(P) -> P, item), 0, &mut cols);
    cols.into_iter()
        .filter_map(|c| {
            let drawn = shape(g, c, (il2cpp::api().object_get_class)(c), &mut 0)?;
            let m = call!(g.tr_matrix, fn(P) -> Mat4, call!(g.comp_transform, fn(P) -> P, c));
            Some((to_pivot.mul(&m), drawn))
        })
        .collect()
}

/// Was something solid for the player in the way between two points (what the push went through)?
unsafe fn blocked(g: &G, from: V3, to: V3, player_layer: i32) -> bool {
    let len = from.dist(to);
    if len < 0.05 {
        return false;
    }
    let mask = collision_mask(g, player_layer, &[player_layer, INSPECT_LAYER]);
    let mut hit = Hit::default();
    let dir = to.sub(from).scale(1.0 / len);
    call!(g.sphere_cast, fn(V3, f32, V3, *mut Hit, f32, i32, i32) -> bool, from, 0.05, dir, &mut hit, len, mask, IGNORE_TRIGGERS)
}

/// Follows the game's item inspection: copies the item's shape for the preview and measures the push.
/// `before`: the player capsule last frame.
unsafe fn track_inspection(g: &G, ins: &Inspection, s: &mut State, cc: P, before: Option<(V3, f32, f32)>) {
    let ui = call!(ins.singleton, fn() -> P);
    let Some((center, radius, height)) = s.capsule else { return };
    if !alive(ui) || !alive(cc) {
        return;
    }
    let layer = call!(g.go_layer, fn(P) -> i32, call!(g.comp_gameobject, fn(P) -> P, cc));
    s.inspect_solid = Some(!call!(g.ignore_layers, fn(i32, i32) -> bool, layer, INSPECT_LAYER));
    if call!(ins.is_inspecting, fn() -> bool) {
        if s.inspect_start.is_none() {
            // Opening frame: the pivot is still where ShowInspectable put it, unrotated.
            let (item, pivot): (P, P) = (rd(ui, ins.off_current), rd(ui, ins.off_pivot));
            if alive(item) && alive(pivot) {
                s.inspect_item = name_of(g, item);
                s.inspect_shapes = item_shapes(g, item, pivot);
            }
            // Last frame's position: the push may already have happened this frame.
            let from = before.map_or(center, |c| c.0);
            let cam = call!(g.player_camera, fn() -> P);
            let f = if alive(cam) {
                call!(g.tr_forward, fn(P) -> V3, call!(g.comp_transform, fn(P) -> P, cam))
            } else {
                V3::default()
            };
            let flat = V3 { y: 0.0, ..f };
            s.inspect_start = Some((from, flat.scale(1.0 / flat.dot(flat).sqrt().max(1e-6))));
        }
    } else if let Some((from, fwd)) = s.inspect_start.take() {
        // First frame after closing it.
        let crossed = blocked(g, from, center, layer);
        s.push = Some(Push { from, to: center, fwd, crossed, radius, height, at: Instant::now() });
    }
}

/// The inspected item where inspecting now would put it, the player's capsule, the last push.
unsafe fn draw_inspect(g: &G, s: &mut State, vp: &Mat4) {
    if g.insp.is_none() {
        return;
    }
    let (mut lines, mut polys, mut labels) = (Vec::new(), Vec::new(), Vec::new());
    let cam = call!(g.player_camera, fn() -> P);
    if s.inspect_preview && !s.inspect_shapes.is_empty() && alive(cam) {
        let tr = call!(g.comp_transform, fn(P) -> P, cam);
        let eye = call!(g.tr_get_pos, fn(P) -> V3, tr);
        let spawn = eye.add_scaled(call!(g.tr_forward, fn(P) -> V3, tr), INSPECT_DISTANCE);
        // ShowInspectable moves the pivot there with no rotation; the item keeps its place in it.
        let to_spawn = Mat4::translation(spawn);
        let color = [0.2, 0.9, 1.0, 1.0];
        for (in_pivot, shape) in &s.inspect_shapes {
            let m = to_spawn.mul(in_pivot);
            for &(a, b) in &shape.edges {
                world_line(&mut lines, vp, m.apply(a), m.apply(b), color);
            }
            for face in &shape.faces {
                let pts = clip_front(&face.map(|p| vp.point(m.apply(p))));
                if pts.len() >= 3 {
                    polys.push(Poly { pts, color: [color[0], color[1], color[2], 0.1] });
                }
            }
        }
        if let Some(p) = vp.project(spawn) {
            labels.push((p, format!("INSPECT : {}", s.inspect_item), color));
        }
        if let Some((c, r, h)) = s.capsule {
            for (a, b) in capsule_shape(c, r, h, 1).edges {
                world_line(&mut lines, vp, a, b, [1.0, 1.0, 1.0, 0.6]);
            }
        }
    }
    if let Some(p) = s.push.filter(|p| p.recent()) {
        let color = p.color();
        world_line(&mut lines, vp, p.from, p.to, color);
        for (a, b) in capsule_shape(p.to, p.radius, p.height, 1).edges {
            world_line(&mut lines, vp, a, b, color);
        }
        if let Some(at) = vp.project(p.to) {
            labels.push((at, p.label(), color));
        }
    }
    s.lines.extend(lines);
    s.polys.extend(polys);
    s.labels.extend(labels);
}

unsafe fn draw_colliders(g: &G, s: &mut State, eye: V3, vp: &Mat4) {
    for col in &s.colliders {
        let info = &col.info;
        let wanted = match info.kind {
            Kind::Solid => s.show_colliders && s.show_visible_walls,
            Kind::Invisible => s.show_colliders,
            Kind::Trigger | Kind::Special => s.show_triggers,
            Kind::Exit => s.show_exits,
            Kind::Ignored => false,
        };
        // Level ends are few and wanted from afar; the rest only around the player.
        let exit = info.kind == Kind::Exit;
        let radius = if exit { f32::INFINITY } else { s.collider_radius };
        // Fading with distance makes what's close stand out from what's behind it.
        let fade = |p: V3| if exit { 1.0 } else { (1.0 - p.dist(eye) / radius).clamp(0.15, 1.0) };
        if !wanted || !alive(col.obj as P) || aabb_dist(&col.bounds, eye) > radius || s.lines.len() >= MAX_LINES {
            continue;
        }
        let model = call!(g.tr_matrix, fn(P) -> Mat4, call!(g.comp_transform, fn(P) -> P, col.obj as P));
        let world = |p: V3| model.apply(p);
        let (color, fill) = match info.kind {
            Kind::Solid => ([0.35, 0.75, 1.0, 0.7], 0.0),
            Kind::Invisible => ([1.0, 0.55, 0.0, 1.0], 0.2),
            Kind::Trigger => ([1.0, 0.95, 0.2, 0.85], 0.08),
            Kind::Exit => ([1.0, 0.3, 1.0, 1.0], 0.25),
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
        if !info.label.is_empty() && (exit || col.bounds.center.dist(eye) < radius.min(15.0)) {
            if let Some(p) = vp.project(col.bounds.center) {
                s.labels.push((p, info.label.clone(), alpha(1.0)));
            }
        }
    }
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

    // Raw object pointers are only safe while Unity keeps the objects: drop them all on level change.
    if s.controller != ctrl as usize {
        s.controller = ctrl as usize;
        (s.colliders, s.infos, s.last_scan) = (Vec::new(), BTreeMap::new(), None);
        (s.floats, s.last_float_scan) = (Vec::new(), None);
        (s.capsule, s.inspect_start) = (None, None);
        (s.cam_saved, s.applied_free_cam) = (None, false);
        (s.float_of_shapes, s.carried_last, s.dropped) = (0, 0, None);
    }

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
    let cam = rd::<P>(ctrl, g.off_camera);
    let free_cam = s.free_cam && alive(cam);
    let cam_tr = if alive(cam) { call!(g.comp_transform, fn(P) -> P, cam) } else { std::ptr::null_mut() };
    if free_cam && !s.applied_free_cam {
        s.cam_pos = call!(g.tr_get_pos, fn(P) -> V3, cam_tr);
        (s.cam_yaw, s.cam_pitch) = yaw_pitch_of(call!(g.tr_forward, fn(P) -> V3, cam_tr));
        (s.pin_pos, s.look_input) = (pos, [0.0; 2]);
    }
    if !free_cam {
        if let Some((p, r)) = s.cam_saved.take().filter(|_| alive(cam)) {
            call!(g.tr_set_pos, fn(P, V3), cam_tr, p);
            call!(g.tr_set_rot, fn(P, Quat), cam_tr, r);
        }
    }
    // The free camera turns with the mouse taken from the player (pc_update), at the game's speed.
    if free_cam {
        let speed = if alive(vars) { rd::<f32>(vars, g.off_look_speed) } else { 1.0 };
        let [dx, dy] = std::mem::take(&mut s.look_input);
        s.cam_yaw = (s.cam_yaw + dx * speed) % 360.0;
        s.cam_pitch = (s.cam_pitch - dy * speed).clamp(-89.0, 89.0);
    }
    // The keys fly the free camera when it's on, the noclip body otherwise.
    let step = if free_cam {
        let (fwd, right) = yaw_pitch_axes(s.cam_yaw, s.cam_pitch);
        fly_step(g, s, fwd, right)
    } else if noclip && alive(cam) {
        fly_step(g, s, call!(g.tr_forward, fn(P) -> V3, cam_tr), call!(g.tr_right, fn(P) -> V3, cam_tr))
    } else {
        V3::default()
    };
    if free_cam {
        s.cam_pos = s.cam_pos.add_scaled(step, 1.0);
    }
    if noclip {
        if !s.applied_noclip {
            s.fly_pos = pos;
        }
        if !free_cam {
            s.fly_pos = s.fly_pos.add_scaled(step, 1.0);
        }
    }
    let hold = if noclip { Some(s.fly_pos) } else if free_cam { Some(s.pin_pos) } else { None };
    if let Some(p) = hold {
        // The CharacterController moved us during Update; override it and push the change to physics.
        call!(g.tr_set_pos, fn(P, V3), tr, p);
        call!(g.sync_transforms, fn());
        wr(ctrl, g.off_gravity, 0.0f32);
    }
    s.applied_noclip = noclip;
    s.applied_free_cam = free_cam;
    s.pos = hold.unwrap_or(pos);
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
        s.last_float_scan = Some(Instant::now());
    }
    let carried =
        s.floats.iter().map(|&f| f as P).find(|&f| my_id != 0 && alive(f) && rd::<u32>(f, g.off_carrier) == my_id);
    let cc = rd::<P>(ctrl, g.off_character);
    if let Some(f) = carried {
        if s.float_of_shapes != f as usize {
            (s.float_shapes, s.float_top) = float_shapes(g, f);
            s.float_of_shapes = f as usize;
        }
        (s.carried_last, s.dropped) = (f as usize, None);
    } else if s.carried_last != 0 {
        // Just dropped it: keep it in sight to grab it back (last frame's preview says where it appeared).
        s.dropped = Some((std::mem::take(&mut s.carried_last), Instant::now()));
        (s.drop_spawn, s.landed, s.drop_trail) = (s.drop.map(|d| d.pos), None, Vec::new());
    }
    // Its path, and where it ends up once settled: followed until LANDED_BY after the drop.
    if let Some((f, t)) = s.dropped.filter(|(f, t)| t.elapsed() <= LANDED_BY && s.floats.contains(f)) {
        let float = f as P;
        let pos = if alive(float) { transform_pos(g, float) } else { None };
        if let Some(pos) = pos {
            s.drop_trail.push(pos);
        }
        if let Some(pos) = pos.filter(|_| alive(cc) && t.elapsed() >= LANDED_AFTER) {
            let player_layer = call!(g.go_layer, fn(P) -> i32, call!(g.comp_gameobject, fn(P) -> P, cc));
            let layer = call!(g.go_layer, fn(P) -> i32, call!(g.comp_gameobject, fn(P) -> P, float));
            let top = roof_margin(g, pos, pos, s.float_top, collision_mask(g, layer, &[player_layer, layer]));
            let fell = s.drop_spawn.map_or(0.0, |p| p.y - pos.y);
            s.landed = Some(Landed { top, fell, at: t });
        }
    }
    (s.drop, s.drop_model) = match carried {
        Some(f) if s.drop_preview && alive(cc) => {
            let layer = call!(g.go_layer, fn(P) -> i32, call!(g.comp_gameobject, fn(P) -> P, cc));
            let (info, model) = drop_preview(g, f, tr, layer, s.float_top);
            (Some(info), Some(model))
        },
        _ => (None, None),
    };
    let before = s.capsule;
    s.capsule = alive(cc).then(|| capsule(g, cc));
    if let (Some(d), Some(body)) = (s.drop.as_mut(), s.capsule) {
        d.on_player = sphere_in_capsule(d.pos, d.radius, body);
    }
    if let Some(ins) = &g.insp {
        track_inspection(g, ins, s, cc, before);
    }

    s.camera = cam as usize;
    if s.show_colliders || s.show_triggers || s.show_exits {
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
        match action {
            Action::Save => {
                let rot = call!(g.tr_get_rot, fn(P) -> Quat, tr);
                let items = if alive(inv) { inventory(g, inv, &s.items) } else { Vec::new() };
                let float = carried.map_or(0, |f| f as usize);
                s.slots.push(Saved { pos: s.pos, rot, stats: s.stats, items, float });
                s.selected = s.slots.len() - 1;
            },
            Action::Load(i) => {
                let Some(saved) = s.slots.get(i).cloned() else { continue };
                s.selected = i;
                s.fly_pos = saved.pos;
                teleport(g, ctrl, saved.pos, saved.rot, false);
                for (off, v) in g.off_stats.iter().zip(saved.stats) {
                    wr(stats, *off, v);
                }
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
        }
    }
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
    let hitboxes = s.show_colliders || s.show_triggers || s.show_exits;
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
            side: "devant",
            top: None,
            on_player: false,
        };
        let (green, orange, red) = ([0.2, 1.0, 0.3, 1.0], [1.0, 0.6, 0.1, 1.0], [1.0, 0.2, 0.2, 1.0]);
        assert_eq!(at(10.0, Some(9.7), false).color(), green); // lands on the roof
        assert_eq!(at(10.0, Some(8.0), false).color(), orange);
        assert_eq!(at(10.0, Some(2.0), false).color(), red); // back into the room
        assert_eq!(at(10.0, None, false).color(), red);
        assert_eq!(at(10.0, Some(9.7), true).color(), orange); // spawns inside the slab
        assert_eq!(at(10.0, Some(2.0), false).label(), "devant : retombe de 8.0 m");
        // The roof part: its top must stick out to grab it back from up there.
        let roof = |top| DropInfo { top: Some(top), ..at(10.0, Some(9.8), false) };
        assert_eq!(roof(0.12).label(), "devant : reste en haut (0.2 m) | depasse du toit de 12 cm");
        assert_eq!(roof(0.12).roof_color(), green);
        assert_eq!(roof(-0.3).label(), "devant : reste en haut (0.2 m) | sous le toit, il manque 30 cm");
        assert_eq!(roof(-0.3).roof_color(), red);
        // Inside the player: the boost, said as such, the roof verdict still there.
        let in_body = DropInfo { on_player: true, ..roof(0.12) };
        assert_eq!(in_body.label(), "devant : dans toi (te pousse) | reste en haut (0.2 m) | depasse du toit de 12 cm");
        assert_eq!(in_body.roof_color(), green);
        // Where it ended up after the drop.
        let landed = |top, fell| Landed { top, fell, at: Instant::now() };
        assert_eq!(landed(Some(0.05), 0.3).label(), "contre le toit, depasse de 5 cm : vise-la d'en haut + E");
        assert_eq!(landed(Some(0.05), 0.3).color(), green);
        assert_eq!(landed(Some(-0.4), 0.3).label(), "restee en haut, 40 cm sous le toit");
        assert_eq!(landed(Some(0.05), 2.5).label(), "tombee de 2.5 m"); // fell off, wherever it is
        assert_eq!(landed(Some(0.05), 2.5).color(), red);
        // Player capsule 1.8 m tall, 0.4 m wide, centered at 1 m; a 0.5 m float around it.
        let body = (V3 { x: 0.0, y: 1.0, z: 0.0 }, 0.4, 1.8);
        assert!(sphere_in_capsule(V3 { x: 0.6, y: 1.5, z: 0.0 }, 0.5, body)); // chest height, beside
        assert!(sphere_in_capsule(V3 { x: 0.0, y: 2.3, z: 0.0 }, 0.5, body)); // over the head
        assert!(!sphere_in_capsule(V3 { x: 1.0, y: 1.5, z: 0.0 }, 0.5, body));
        assert!(!sphere_in_capsule(V3 { x: 0.0, y: 2.8, z: 0.0 }, 0.5, body));
        // Facing +Z: right is +X.
        let (fwd, right) = (V3 { x: 0.0, y: 0.0, z: 1.0 }, V3 { x: 1.0, y: 0.0, z: 0.0 });
        let side = |x: f32, z: f32| side_of(V3 { x, y: 1.5, z }, fwd, right);
        assert_eq!(side(0.1, 0.8), "devant");
        assert_eq!(side(0.0, -0.6), "derriere");
        assert_eq!(side(0.7, 0.2), "a droite");
        assert_eq!(side(-0.7, 0.2), "a gauche");
        assert_eq!(side(0.01, -0.02), "au-dessus de toi");
    }

    #[test]
    fn inspect_push_relative_to_facing() {
        // Facing +X: right is -Z in Unity's left-handed axes.
        let push = |to: V3, crossed| Push {
            from: V3::default(),
            to,
            fwd: V3 { x: 1.0, y: 0.0, z: 0.0 },
            crossed,
            radius: 0.3,
            height: 1.8,
            at: Instant::now(),
        };
        let through = push(V3 { x: 0.8, y: 0.0, z: -0.1 }, true);
        assert_eq!(through.label(), "pousse de 0.81 m (avant +0.80, droite +0.10, haut +0.00) : A TRAVERSE");
        assert_eq!(through.color(), [0.2, 1.0, 0.3, 1.0]);
        assert_eq!(push(V3 { x: -0.5, y: 0.0, z: 0.0 }, false).color(), [1.0, 0.6, 0.1, 1.0]);
        assert_eq!(push(V3 { x: 0.01, y: 0.0, z: 0.0 }, true).label(), "pas pousse");
        let p = Mat4::translation(V3 { x: 1.0, y: 2.0, z: 3.0 }).apply(V3 { x: 1.0, y: 1.0, z: 1.0 });
        assert_eq!(p, V3 { x: 2.0, y: 3.0, z: 4.0 });
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
    fn free_camera_rotation() {
        let near = |a: V3, b: V3| a.dist(b) < 1e-4;
        let (fwd, right) = yaw_pitch_axes(0.0, 0.0);
        assert!(near(fwd, V3 { x: 0.0, y: 0.0, z: 1.0 }) && near(right, V3 { x: 1.0, y: 0.0, z: 0.0 }));
        let (fwd, right) = yaw_pitch_axes(90.0, 0.0);
        assert!(near(fwd, V3 { x: 1.0, y: 0.0, z: 0.0 }) && near(right, V3 { x: 0.0, y: 0.0, z: -1.0 }));
        assert!(near(yaw_pitch_axes(0.0, 90.0).0, V3 { x: 0.0, y: -1.0, z: 0.0 })); // pitch > 0 looks down
        let (yaw, pitch) = yaw_pitch_of(yaw_pitch_axes(30.0, -20.0).0);
        assert!((yaw - 30.0).abs() < 1e-3 && (pitch + 20.0).abs() < 1e-3);
        // The rotation set on the camera looks where the free camera moves.
        let rotate = |q: Quat, v: V3| {
            let (u, w) = (V3 { x: q.0[0], y: q.0[1], z: q.0[2] }, q.0[3]);
            let t = u.cross(v).scale(2.0);
            v.add_scaled(t, w).add_scaled(u.cross(t), 1.0)
        };
        for (yaw, pitch) in [(0.0, 0.0), (90.0, 0.0), (-135.0, 40.0), (200.0, -70.0)] {
            let q = yaw_pitch_rotation(yaw, pitch);
            let (fwd, right) = yaw_pitch_axes(yaw, pitch);
            assert!(near(rotate(q, AXES[2]), fwd) && near(rotate(q, AXES[0]), right), "{yaw} {pitch}");
        }
    }

    #[test]
    fn capsule_spans_its_height() {
        let s = capsule_shape(V3::default(), 0.5, 3.0, 1);
        let top = s.edges.iter().flat_map(|e| [e.0.y, e.1.y]).fold(f32::MIN, f32::max);
        let bottom = s.edges.iter().flat_map(|e| [e.0.y, e.1.y]).fold(f32::MAX, f32::min);
        assert!((top - 1.5).abs() < 1e-4 && (bottom + 1.5).abs() < 1e-4);
    }
}
