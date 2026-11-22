// The game's methods and fields the bot uses, resolved by name (works on 0.7.35 and 0.8.0), and the
// little 3D math it needs.
use crate::call;
use crate::il2cpp::{self, alive, class, field, method, method_sig, rd, M, P};

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct V3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl V3 {
    pub const UP: V3 = V3 { x: 0.0, y: 1.0, z: 0.0 };
    pub const DOWN: V3 = V3 { x: 0.0, y: -1.0, z: 0.0 };

    pub fn add_scaled(self, o: V3, k: f32) -> V3 {
        V3 { x: self.x + o.x * k, y: self.y + o.y * k, z: self.z + o.z * k }
    }

    pub fn scale(self, k: f32) -> V3 {
        V3::default().add_scaled(self, k)
    }

    pub fn cross(self, o: V3) -> V3 {
        V3 { x: self.y * o.z - self.z * o.y, y: self.z * o.x - self.x * o.z, z: self.x * o.y - self.y * o.x }
    }

    pub fn dot(self, o: V3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    pub fn sub(self, o: V3) -> V3 {
        self.add_scaled(o, -1.0)
    }

    pub fn dist(self, o: V3) -> f32 {
        ((self.x - o.x).powi(2) + (self.y - o.y).powi(2) + (self.z - o.z).powi(2)).sqrt()
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct Quat(pub [f32; 4]);

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Bounds {
    pub center: V3,
    pub extents: V3,
}

/// UnityEngine.RaycastHit: only the distance is read; the rest just needs room.
#[repr(C)]
#[derive(Default)]
#[allow(dead_code)]
struct Hit {
    point: V3,
    normal: V3,
    face: u32,
    distance: f32,
    rest: [u32; 24],
}

/// Unity's Quaternion.Euler(0, yaw, 0), in degrees.
pub fn yaw_rotation(yaw: f32) -> Quat {
    let (s, c) = (yaw.to_radians() / 2.0).sin_cos();
    Quat([0.0, s, 0.0, c])
}

/// Forward and right of a yaw.
pub fn yaw_axes(yaw: f32) -> (V3, V3) {
    let (s, c) = yaw.to_radians().sin_cos();
    (V3 { x: s, y: 0.0, z: c }, V3 { x: c, y: 0.0, z: -s })
}

/// `v` turned by `q`.
pub fn rotate(q: Quat, v: V3) -> V3 {
    let (u, w) = (V3 { x: q.0[0], y: q.0[1], z: q.0[2] }, q.0[3]);
    let t = u.cross(v).scale(2.0);
    v.add_scaled(t, w).add_scaled(u.cross(t), 1.0)
}

/// Yaw (degrees) of a rotation's forward.
pub fn yaw_of(q: Quat) -> f32 {
    let f = rotate(q, V3 { x: 0.0, y: 0.0, z: 1.0 });
    f.x.atan2(f.z).to_degrees()
}

/// QueryTriggerInteraction.Ignore
const IGNORE_TRIGGERS: i32 = 1;

/// Main menu: what Continue does (0.8.0+: none in 0.7.35, which has no saved matches).
pub struct Menu {
    pub update: M,
    pub refresh: M,
    pub continue_game: M,
    pub off_saved: usize,
}

/// Running faster than real time with the same physics as at 60 FPS: each frame advances the game a
/// fixed 1/60 s (Time.captureDeltaTime) and frames come as fast as the PC makes them (no frame cap,
/// no vsync), most of them not drawn (OnDemandRendering).
pub struct Turbo {
    pub capture_dt: M,
    pub fps_get: M,
    pub fps_set: M,
    pub vsync_set: M,
    /// Only the engine's own getter is left in the build.
    pub vsync_get: Option<unsafe extern "C" fn() -> i32>,
    pub interval_get: M,
    /// OnDemandRendering.m_RenderFrameInterval: its setter was stripped, the engine reads the field.
    pub interval: P,
}

pub struct G {
    pub menu: Option<Menu>,
    pub turbo: Option<Turbo>,
    /// PlayerController.InputRunningHold: the sprint key, held.
    pub off_run_hold: usize,
    /// BasePlayerController.groundedChecker and its IsGrounded.
    pub off_grounded_checker: usize,
    pub off_is_grounded: usize,
    pub player_stats: M,
    pub add_stamina: M,
    /// PlayerController.InputMoveDir: the move stick, read during its Update (set by OnMoveInput).
    pub off_move_dir: usize,
    pub player_controller: M,
    pub player_camera: M,
    pub comp_transform: M,
    pub comp_gameobject: M,
    pub get_component: M,
    pub obj_name: M,
    pub find_objects: M,
    pub tr_get_pos: M,
    pub tr_get_rot: M,
    pub go_layer: M,
    pub delta_time: M,
    pub time_scale_get: M,
    pub time_scale_set: M,
    pub run_in_background: M,
    pub ignore_layers: M,
    pub sphere_cast: M,
    pub col_bounds: M,
    pub teleport: M,
    /// Teleport takes holdForGround (0.8.0+), not before.
    pub teleport_hold: bool,
    pub start_jump: M,
    pub off_gravity: usize,
    pub net_id: M,
    pub t_identity: P,
    pub carry_drop: M,
    pub carry_pickup: M,
    pub off_carrier: usize,
    pub off_colliders: usize,
    pub t_carry: P,
    pub pc_update: M,
}

unsafe impl Send for G {}
unsafe impl Sync for G {}

pub unsafe fn resolve() -> Result<G, String> {
    let (ac, core, phys) = ("Assembly-CSharp.dll", "UnityEngine.CoreModule.dll", "UnityEngine.PhysicsModule.dll");
    let k = |name: &str| class(ac, "", name);
    let ue = |name: &str| class(core, "UnityEngine", name);
    let (player, bpc, pc) = (k("Player")?, k("BasePlayerController")?, k("PlayerController")?);
    let carry = class(ac, "ITB.SYSTEMS.POOLROOMS", "PoolroomsFloaterCarry")?;
    let identity = class("Mirror.dll", "Mirror", "NetworkIdentity")?;
    let (object, comp, tr, go, time, app) =
        (ue("Object")?, ue("Component")?, ue("Transform")?, ue("GameObject")?, ue("Time")?, ue("Application")?);
    let (physics, collider) = (class(phys, "UnityEngine", "Physics")?, class(phys, "UnityEngine", "Collider")?);
    let (teleport, teleport_hold) = match method(bpc, "Teleport", 3) {
        Ok(m) => (m, true),
        Err(_) => (method(bpc, "Teleport", 2)?, false),
    };
    let main_menu = k("MainMenuUI")?;
    let quality = ue("QualitySettings")?;
    let on_demand = class(core, "UnityEngine.Rendering", "OnDemandRendering")?;
    Ok(G {
        turbo: (|| {
            let interval_get = method(on_demand, "get_renderFrameInterval", 0).ok()?;
            // Initializes the class, so its static field can be written.
            call!(interval_get, fn() -> i32);
            Some(Turbo {
                capture_dt: method(time, "set_captureDeltaTime", 1).ok()?,
                fps_get: method(app, "get_targetFrameRate", 0).ok()?,
                fps_set: method(app, "set_targetFrameRate", 1).ok()?,
                vsync_set: method(quality, "set_vSyncCount", 1).ok()?,
                vsync_get: il2cpp::icall("UnityEngine.QualitySettings::get_vSyncCount()").map(|f| std::mem::transmute::<P, unsafe extern "C" fn() -> i32>(f)),
                interval_get,
                interval: il2cpp::static_field(on_demand, "m_RenderFrameInterval").ok()?,
            })
        })(),
        off_run_hold: field(pc, "InputRunningHold")?,
        off_grounded_checker: field(bpc, "groundedChecker")?,
        off_is_grounded: field(k("GroundedChecker")?, "<IsGrounded>k__BackingField")?,
        player_stats: method(player, "get_Stats", 0)?,
        add_stamina: method(k("PlayerStats")?, "IncreaseStamina", 1)?,
        menu: (|| {
            Some(Menu {
                update: method(main_menu, "Update", 0).ok()?,
                refresh: method(main_menu, "RefreshSavedMatch", 0).ok()?,
                continue_game: method(main_menu, "ContinueSavedGame", 3).ok()?,
                off_saved: field(main_menu, "m_SavedMatch").ok()?,
            })
        })(),
        off_move_dir: field(pc, "InputMoveDir")?,
        player_controller: method(player, "get_Controller", 0)?,
        player_camera: method(player, "get_Camera", 0)?,
        comp_transform: method(comp, "get_transform", 0)?,
        comp_gameobject: method(comp, "get_gameObject", 0)?,
        get_component: method_sig(comp, "GetComponent", &["System.Type"])?,
        obj_name: method(object, "get_name", 0)?,
        find_objects: method_sig(object, "FindObjectsByType", &["System.Type", "UnityEngine.FindObjectsSortMode"])?,
        tr_get_pos: method(tr, "get_position", 0)?,
        tr_get_rot: method(tr, "get_rotation", 0)?,
        go_layer: method(go, "get_layer", 0)?,
        delta_time: method(time, "get_deltaTime", 0)?,
        time_scale_get: method(time, "get_timeScale", 0)?,
        time_scale_set: method(time, "set_timeScale", 1)?,
        run_in_background: method(app, "set_runInBackground", 1)?,
        ignore_layers: method(physics, "GetIgnoreLayerCollision", 2)?,
        sphere_cast: method_sig(
            physics,
            "SphereCast",
            &[
                "UnityEngine.Vector3", "System.Single", "UnityEngine.Vector3", "UnityEngine.RaycastHit&", "System.Single",
                "System.Int32", "UnityEngine.QueryTriggerInteraction",
            ],
        )?,
        col_bounds: method(collider, "get_bounds", 0)?,
        teleport,
        teleport_hold,
        start_jump: method(pc, "StartJump", 0)?,
        off_gravity: field(pc, "m_GravityForce")?,
        net_id: method(identity, "get_netId", 0)?,
        t_identity: il2cpp::type_object(identity),
        carry_drop: method(carry, "UserCode_CmdDrop__NetworkIdentity", 1)?,
        carry_pickup: method(carry, "UserCode_CmdPickup__NetworkIdentity", 1)?,
        off_carrier: field(carry, "m_CarrierNetId")?,
        off_colliders: field(carry, "m_Colliders")?,
        t_carry: il2cpp::type_object(carry),
        pc_update: method(pc, "Update", 0)?,
    })
}

impl G {
    pub unsafe fn pos(&self, comp: P) -> V3 {
        call!(self.tr_get_pos, fn(P) -> V3, call!(self.comp_transform, fn(P) -> P, comp))
    }

    pub unsafe fn layer(&self, comp: P) -> i32 {
        call!(self.go_layer, fn(P) -> i32, call!(self.comp_gameobject, fn(P) -> P, comp))
    }

    pub unsafe fn name(&self, comp: P) -> String {
        il2cpp::string(call!(self.obj_name, fn(P) -> P, call!(self.comp_gameobject, fn(P) -> P, comp)))
            .replace("(Clone)", "")
            .trim()
            .to_string()
    }

    pub unsafe fn teleport(&self, ctrl: P, pos: V3, rot: Quat) {
        if self.teleport_hold {
            call!(self.teleport, fn(P, V3, Quat, bool), ctrl, pos, rot, false)
        } else {
            call!(self.teleport, fn(P, V3, Quat), ctrl, pos, rot)
        }
    }

    pub unsafe fn grounded(&self, ctrl: P) -> bool {
        let checker: P = rd(ctrl, self.off_grounded_checker);
        alive(checker) && rd::<bool>(checker, self.off_is_grounded)
    }

    /// Back to full stamina (the game caps it), so every try can sprint.
    pub unsafe fn refill_stamina(&self) {
        let stats = call!(self.player_stats, fn() -> P);
        if alive(stats) {
            call!(self.add_stamina, fn(P, f32), stats, 1000.0);
        }
    }

    /// How far back from `base` (up to `want`, against `fwd`) a run-up can start: clear of walls
    /// and with floor under it at the same height (not the pool).
    pub unsafe fn runup_room(&self, base: V3, fwd: V3, want: f32, mask: i32) -> f32 {
        let back = fwd.scale(-1.0);
        if want <= 0.0 {
            // Starting ahead: only needs floor there.
            return if self.floor_near(base.add_scaled(back, want), mask) { want } else { 0.0 };
        }
        let mut d = want;
        while d > 0.05 {
            let at = base.add_scaled(back, d);
            let clear = self.ray(base.add_scaled(V3::UP, 1.0), back, d + 0.4, mask).is_none();
            if clear && self.floor_near(at, mask) {
                return d;
            }
            d -= 0.25;
        }
        0.0
    }

    /// Floor within half a metre of `at`'s height.
    pub unsafe fn floor_near(&self, at: V3, mask: i32) -> bool {
        self.ray(at.add_scaled(V3::UP, 0.5), V3::DOWN, 1.0, mask).is_some()
    }

    /// Every Poolrooms float in the level.
    pub unsafe fn floats(&self) -> Vec<P> {
        il2cpp::array(call!(self.find_objects, fn(P, i32) -> P, self.t_carry, 0))
    }

    /// The local player's network id (a float's carrier when they hold it).
    pub unsafe fn my_net_id(&self, ctrl: P) -> (P, u32) {
        let identity = call!(self.get_component, fn(P, P) -> P, ctrl, self.t_identity);
        (identity, if alive(identity) { call!(self.net_id, fn(P) -> u32, identity) } else { 0 })
    }

    pub unsafe fn carrier(&self, float: P) -> u32 {
        rd(float, self.off_carrier)
    }

    /// How high the float's colliders reach (enabled only once dropped).
    pub unsafe fn float_top(&self, float: P) -> Option<f32> {
        il2cpp::array(rd(float, self.off_colliders))
            .into_iter()
            .filter(|&c| alive(c))
            .map(|c| {
                let b = call!(self.col_bounds, fn(P) -> Bounds, c);
                b.center.y + b.extents.y
            })
            .reduce(f32::max)
    }

    /// Layer mask of what `layer` collides with, minus `skip`.
    pub unsafe fn mask(&self, layer: i32, skip: &[i32]) -> i32 {
        (0..32)
            .filter(|l| !skip.contains(l) && !call!(self.ignore_layers, fn(i32, i32) -> bool, layer, *l))
            .fold(0i32, |m, l| m | 1 << l)
    }

    /// Distance to the first solid thing for `mask` along a ray (a thin sphere cast).
    pub unsafe fn ray(&self, from: V3, dir: V3, max: f32, mask: i32) -> Option<f32> {
        let mut hit = Hit::default();
        call!(self.sphere_cast, fn(V3, f32, V3, *mut Hit, f32, i32, i32) -> bool, from, 0.02, dir, &mut hit, max, mask, IGNORE_TRIGGERS)
            .then_some(hit.distance)
    }

    /// The roof's upper face over `at`, found from above the ceiling (the ceiling itself if none).
    pub unsafe fn roof_top(&self, at: V3, ceiling: f32, mask: i32) -> f32 {
        let above = V3 { y: ceiling + 3.0, ..at };
        self.ray(above, V3::DOWN, 3.5, mask).map_or(ceiling, |d| above.y - d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaw_math() {
        let near = |a: V3, b: V3| a.dist(b) < 1e-4;
        for yaw in [0.0, 90.0, -135.0, 200.0] {
            let (fwd, right) = yaw_axes(yaw);
            let q = yaw_rotation(yaw);
            assert!(near(rotate(q, V3 { x: 0.0, y: 0.0, z: 1.0 }), fwd), "{yaw}");
            assert!(near(rotate(q, V3 { x: 1.0, y: 0.0, z: 0.0 }), right), "{yaw}");
            let back = yaw_of(q);
            assert!(((back - yaw).rem_euclid(360.0) + 1e-3) % 360.0 < 2e-3, "{yaw} {back}");
        }
        // Facing +X: right is -Z in Unity's left-handed axes.
        assert!(near(yaw_axes(90.0).1, V3 { x: 0.0, y: 0.0, z: -1.0 }));
    }
}
