// Inside the Backrooms practice tool: DLL injected by the launcher, draws an imgui overlay
// (hudhook) and drives the game through its own il2cpp methods (see game.rs).
mod config;
mod game;
mod il2cpp;

use std::ffi::c_void;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::OnceLock;
use std::time::Duration;

use hudhook::hooks::dx11::ImguiDx11Hooks;
use hudhook::hooks::dx12::ImguiDx12Hooks;
use hudhook::imgui::{Condition, Context, Io, TreeNodeFlags, Ui, WindowFlags};
use hudhook::windows::core::PCSTR;
use hudhook::windows::Win32::Foundation::{HINSTANCE, HMODULE};
use hudhook::windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleA};
use hudhook::windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use hudhook::windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
use hudhook::{Hudhook, ImguiRenderLoop, MessageFilter, RenderContext};

use game::{lock, Action, GOD, INF_ITEMS, NOCLIP, NO_FALL_TP};

static LOG: OnceLock<PathBuf> = OnceLock::new();
static INIT_ERROR: OnceLock<String> = OnceLock::new();
static KEYS: OnceLock<config::Keys> = OnceLock::new();

fn log(msg: &str) {
    if let Some(Ok(mut f)) = LOG.get().map(|p| std::fs::OpenOptions::new().create(true).append(true).open(p)) {
        let _ = writeln!(f, "{msg}");
    }
}

pub fn game_focused() -> bool {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut pid)) };
    pid == std::process::id()
}

/// " [F2]" for a bound action, "" otherwise.
fn hk(action: usize) -> String {
    KEYS.get().and_then(|k| k[action]).map_or(String::new(), |vk| format!(" [{}]", config::key_name(vk)))
}

struct Overlay {
    prev_keys: [bool; config::ACTIONS.len()],
    item_filter: String,
}

impl Overlay {
    /// Edge-triggered global hotkeys, only while the game window has focus.
    fn hotkeys(&mut self) {
        let focused = game_focused();
        let Some(keys) = KEYS.get() else { return };
        for (i, vk) in keys.iter().enumerate() {
            let down = focused && vk.is_some_and(|vk| unsafe { GetAsyncKeyState(vk as i32) } as u16 & 0x8000 != 0);
            if down && !self.prev_keys[i] {
                let mut s = lock();
                let s = &mut *s;
                let flip = |b: &AtomicBool| _ = b.fetch_xor(true, Relaxed);
                match config::ACTIONS[i].0 {
                    "menu" => s.menu_open = !s.menu_open,
                    "noclip" => flip(&NOCLIP),
                    "god" => flip(&GOD),
                    "entites" => s.freeze_entities = !s.freeze_entities,
                    "sauver" => s.actions.push(Action::Save),
                    "charger" => s.actions.push(Action::Load(s.selected)),
                    "effets_off" => s.fx_off = !s.fx_off,
                    "colliders" => s.show_colliders = !s.show_colliders,
                    "triggers" => s.show_triggers = !s.show_triggers,
                    "stamina" => s.inf_stamina = !s.inf_stamina,
                    "items_infinis" => flip(&INF_ITEMS),
                    "anti_chute_off" => flip(&NO_FALL_TP),
                    "slot_precedent" => s.selected = s.selected.saturating_sub(1),
                    "slot_suivant" => s.selected = (s.selected + 1).min(s.slots.len().saturating_sub(1)),
                    "fins_niveau" => s.show_exits = !s.show_exits,
                    "camera_libre" => s.free_cam = !s.free_cam,
                    _ => {},
                }
            }
            self.prev_keys[i] = down;
        }
    }
}

fn atomic_checkbox(ui: &Ui, label: &str, flag: &AtomicBool) {
    let mut v = flag.load(Relaxed);
    if ui.checkbox(label, &mut v) {
        flag.store(v, Relaxed);
    }
}

fn section(ui: &Ui, label: &str, open: bool) -> bool {
    ui.collapsing_header(label, if open { TreeNodeFlags::DEFAULT_OPEN } else { TreeNodeFlags::empty() })
}

impl ImguiRenderLoop for Overlay {
    fn initialize<'a>(&'a mut self, ctx: &mut Context, _: &'a mut dyn RenderContext) {
        ctx.set_ini_filename(None::<PathBuf>);
    }

    fn before_render<'a>(&'a mut self, ctx: &mut Context, _: &'a mut dyn RenderContext) {
        ctx.io_mut().mouse_draw_cursor = lock().menu_open;
    }

    fn render(&mut self, ui: &mut Ui) {
        self.hotkeys();
        let mut guard = lock();
        let s = &mut *guard;
        let in_game = s.last_tick.is_some_and(|t| t.elapsed() < Duration::from_secs(1));

        // Collider shapes, in normalized device coordinates from the game thread.
        if in_game && !s.lines.is_empty() {
            let [w, h] = ui.io().display_size;
            let to_screen = |p: [f32; 2]| [(p[0] * 0.5 + 0.5) * w, (0.5 - p[1] * 0.5) * h];
            let dl = ui.get_background_draw_list();
            for p in &s.polys {
                dl.add_polyline(p.pts.iter().map(|&q| to_screen(q)).collect(), p.color).filled(true).build();
            }
            for l in &s.lines {
                dl.add_line(to_screen(l.a), to_screen(l.b), l.color).thickness(1.5).build();
            }
            for (p, label, color) in &s.labels {
                let [x, y] = to_screen(*p);
                dl.add_text([x + 1.0, y + 1.0], [0.0, 0.0, 0.0, 1.0], label);
                dl.add_text([x, y], *color, label);
            }
        }

        ui.window("##hud")
            .position([10.0, 10.0], Condition::Always)
            .bg_alpha(0.45)
            .flags(WindowFlags::NO_DECORATION | WindowFlags::ALWAYS_AUTO_RESIZE | WindowFlags::NO_INPUTS | WindowFlags::NO_NAV)
            .build(|| {
                let flags: Vec<&str> = [
                    (NOCLIP.load(Relaxed), "NOCLIP"),
                    (s.free_cam, "CAMERA LIBRE"),
                    (GOD.load(Relaxed), "GOD"),
                    (s.inf_stamina, "STAMINA"),
                    (INF_ITEMS.load(Relaxed), "ITEMS"),
                    (s.freeze_entities, "ENTITES OFF"),
                    (NO_FALL_TP.load(Relaxed), "ANTI-CHUTE OFF"),
                    (s.time_scale != 1.0, "VITESSE JEU"),
                    (s.speed_mult != 1.0 || s.jump_mult != 1.0, "MOUVEMENT"),
                ]
                .into_iter()
                .filter_map(|(on, n)| on.then_some(n))
                .collect();
                ui.text(format!("Practice Tool{}  {}", hk(0), flags.join(" ")));
                if in_game {
                    let t = s.game_time.max(0.0);
                    ui.text(format!("X {:.2}  Y {:.2}  Z {:.2}", s.pos.x, s.pos.y, s.pos.z));
                    ui.text(format!("Temps {:02}:{:06.3}", (t / 60.0) as u32, t % 60.0));
                    // The game's anti-fall teleports after 5 s airborne with nothing below.
                    let danger = !s.ground_below && s.fall_time > 0.0;
                    ui.text_colored(
                        if danger { [1.0, 0.3, 0.3, 1.0] } else { [0.7, 0.7, 0.7, 1.0] },
                        format!(
                            "En l'air {:.1}s / 5.0s  sol dessous : {}",
                            s.fall_time,
                            if s.ground_below { "oui" } else { "NON" }
                        ),
                    );
                    if let Some(d) = s.drop {
                        ui.text_colored(d.color(), format!("Drop bouee : {}", d.label()));
                    }
                    if let Some(p) = s.push.filter(|p| p.recent()) {
                        ui.text_colored(p.color(), format!("Inspection : {}", p.label()));
                    }
                    if let Some(l) = s.landed.filter(|l| l.recent()) {
                        ui.text_colored(l.color(), format!("Bouee lachee : {}", l.label()));
                    }
                }
            });

        if !s.menu_open {
            return;
        }
        ui.window("Backrooms Practice Tool")
            .position([10.0, 110.0], Condition::FirstUseEver)
            .size([380.0, 640.0], Condition::FirstUseEver)
            .build(|| {
                if let Some(e) = INIT_ERROR.get() {
                    ui.text_colored([1.0, 0.3, 0.3, 1.0], format!("Erreur : {e}"));
                    return;
                }
                if !in_game {
                    ui.text_colored([1.0, 0.8, 0.2, 1.0], "En attente du joueur (lance une partie).");
                }

                if section(ui, "Joueur", true) {
                    atomic_checkbox(ui, &format!("Noclip / vol{}", hk(1)), &NOCLIP);
                    ui.slider("Vitesse vol", 1.0, 50.0, &mut s.fly_speed);
                    ui.text_disabled("ZQSD, Espace monte, Ctrl descend, Shift x3");
                    ui.checkbox(format!("Camera libre{}", hk(15)), &mut s.free_cam);
                    ui.text_disabled("Le joueur reste fige, sans tourner (capsule blanche) :");
                    ui.text_disabled("la souris et les touches du vol bougent la camera.");
                    atomic_checkbox(ui, &format!("God mode{}", hk(2)), &GOD);
                    ui.checkbox(format!("Stamina infinie{}", hk(9)), &mut s.inf_stamina);
                    ui.checkbox("Oxygene infini", &mut s.inf_oxygen);
                    ui.checkbox("Pas d'anxiete", &mut s.no_anxiety);
                    ui.slider("Vitesse deplacement", 0.5, 5.0, &mut s.speed_mult);
                    ui.slider("Hauteur de saut", 0.5, 5.0, &mut s.jump_mult);
                    if ui.small_button("Reset mouvement") {
                        (s.speed_mult, s.jump_mult) = (1.0, 1.0);
                    }
                    atomic_checkbox(ui, &format!("Desactiver le TP anti-chute{}", hk(11)), &NO_FALL_TP);
                    let [h, st, o, a] = s.stats;
                    ui.text(format!("Vie {h:.0}  Stamina {st:.0}  Oxygene {o:.0}  Anxiete {a:.0}"));
                }

                if section(ui, "Monde / visuel", false) {
                    ui.slider("Vitesse du jeu", 0.1, 2.0, &mut s.time_scale);
                    ui.same_line();
                    if ui.small_button("1x") {
                        s.time_scale = 1.0;
                    }
                    ui.checkbox(format!("Effets VHS / camera off{}", hk(6)), &mut s.fx_off);
                    ui.checkbox("Post-process du niveau off", &mut s.level_post_off);
                    ui.slider("Luminosite (EV)", 0.0, 8.0, &mut s.brightness);
                }

                if section(ui, "Hitbox / colliders", false) {
                    ui.checkbox(format!("Murs invisibles{}", hk(7)), &mut s.show_colliders);
                    ui.same_line();
                    ui.checkbox("+ decor et objets", &mut s.show_visible_walls);
                    ui.checkbox(format!("Zones / triggers{}", hk(8)), &mut s.show_triggers);
                    ui.checkbox(format!("Fins de niveau{}", hk(14)), &mut s.show_exits);
                    ui.slider("Rayon (m)", 5.0, 60.0, &mut s.collider_radius);
                    ui.text_colored([1.0, 0.55, 0.0, 1.0], "orange : mur invisible (BoundsCollider...)");
                    ui.text_colored([0.35, 0.75, 1.0, 1.0], "bleu : vraie collision du decor / objets");
                    ui.text_colored([1.0, 0.95, 0.2, 1.0], "jaune : zone (son nom s'affiche de pres)");
                    ui.text_colored([1.0, 0.2, 0.2, 1.0], "rouge : TP / sortie / degats / niveau...");
                    ui.text_colored([1.0, 0.3, 1.0, 1.0], "violet : ce qui finit le niveau, vu de loin");
                    ui.text_disabled("Ce que le joueur traverse n'est pas affiche.");
                    ui.text_disabled("Distorsion / VHS coupes pendant l'affichage.");
                    if s.show_colliders || s.show_triggers {
                        ui.text_disabled(format!("{} colliders proches", s.collider_count));
                        if s.approx_count > 0 {
                            ui.same_line();
                            ui.text_disabled(format!("({} en forme approx.)", s.approx_count));
                        }
                    }
                    if s.show_exits {
                        ui.text_disabled(format!("{} fins de niveau chargees", s.exit_count));
                    }
                }

                if section(ui, "Bouee (Poolrooms)", false) {
                    ui.text_colored([0.2, 1.0, 0.3, 1.0], "Record (saopy) : Q, puis E aussitot : elle monte");
                    ui.text_colored([0.2, 1.0, 0.3, 1.0], "avec toi. Q de nouveau EN traversant le plafond :");
                    ui.text_colored([0.2, 1.0, 0.3, 1.0], "elle reste dans le toit, reprends-la d'en haut (E).");
                    ui.text_disabled("Pour le timing : Monde / visuel > Vitesse du jeu.");
                    ui.text_disabled("Forme rouge : elle restera sous le toit. Une fois");
                    ui.text_disabled("lachee, elle reste dessinee en violet (60 s) avec");
                    ui.text_disabled("son trajet, et l'outil dit ou elle a fini (HUD).");
                    ui.checkbox("Previsualiser le drop", &mut s.drop_preview);
                    ui.text_disabled("Quand tu portes une bouee : la sphere montre ou elle");
                    ui.text_disabled("apparait si tu laches maintenant, le trait ou elle tombe.");
                    ui.text_disabled("Elle apparait a hauteur de tes pieds + dropHeight,");
                    ui.text_disabled("devant toi, sinon a droite, derriere ou a gauche");
                    ui.text_disabled("(selon ton corps, pas ta camera).");
                    ui.text_colored([0.2, 1.0, 0.3, 1.0], "vert : elle reste a cette hauteur");
                    ui.text_colored([1.0, 0.6, 0.1, 1.0], "orange : dans le decor ou petite chute");
                    ui.text_disabled("\"dans toi\" : elle apparait dans ton corps et te pousse.");
                    if let Some(l) = s.landed {
                        ui.text_colored(l.color(), format!("Dernier lacher : {}", l.label()));
                    }
                    ui.text_colored([1.0, 0.2, 0.2, 1.0], "rouge : elle retombe (dans la salle)");
                    ui.text_disabled("Une savestate faite en portant la bouee te la rend.");
                }

                if section(ui, "Glitch d'inspection", false) {
                    ui.checkbox("Apercu de l'objet inspecte", &mut s.inspect_preview);
                    ui.text_disabled("Inspecter un objet le fait apparaitre 10 cm devant la");
                    ui.text_disabled("camera, solide et toujours oriente pareil (axes du");
                    ui.text_disabled("monde) : le jeu te repousse hors de lui, parfois a");
                    ui.text_disabled("travers une grille. Gros objet = grosse poussee.");
                    if s.items.iter().any(|n| n == "ProtectionSuit") && ui.small_button("Donner ProtectionSuit") {
                        s.actions.push(Action::Give("ProtectionSuit".into()));
                    }
                    if s.inspect_solid == Some(false) {
                        ui.text_colored([1.0, 0.3, 0.3, 1.0], "Ici les objets inspectes ne touchent pas le joueur.");
                    }
                    if s.inspect_item.is_empty() {
                        ui.text_colored([1.0, 0.8, 0.2, 1.0], "Inspecte l'objet une fois pour voir sa forme");
                        ui.text_colored([1.0, 0.8, 0.2, 1.0], "(inventaire, clic droit, Inspect).");
                    } else {
                        ui.text(format!("Objet : {}", s.inspect_item));
                        ui.text_colored([0.2, 0.9, 1.0, 1.0], "cyan : ou il apparait si tu inspectes maintenant");
                        ui.text_colored([1.0, 1.0, 1.0, 1.0], "blanc : ta capsule de collision");
                    }
                    ui.text_colored([0.2, 1.0, 0.3, 1.0], "vert : la poussee a traverse un mur");
                    ui.text_colored([1.0, 0.6, 0.1, 1.0], "orange : poussee sans traverser");
                    if let Some(p) = s.push {
                        ui.text_colored(p.color(), format!("Derniere : {}", p.label()));
                    }
                }

                if section(ui, "Positions (savestates)", true) {
                    ui.checkbox("Restaurer l'inventaire au chargement", &mut s.restore_inventory);
                    if ui.button(format!("Sauver{}", hk(4))) {
                        s.actions.push(Action::Save);
                    }
                    ui.same_line();
                    if ui.button(format!("Charger{}", hk(5))) {
                        s.actions.push(Action::Load(s.selected));
                    }
                    let mut remove = None;
                    for i in 0..s.slots.len() {
                        let _id = ui.push_id_usize(i);
                        let (p, n) = (s.slots[i].pos, s.slots[i].items.iter().map(|(_, c)| c).sum::<i32>());
                        let float = if s.slots[i].float != 0 { " + bouee" } else { "" };
                        if ui.radio_button_bool(format!("#{}  {:.1} / {:.1} / {:.1}  ({n} items{float})", i + 1, p.x, p.y, p.z), s.selected == i) {
                            s.selected = i;
                        }
                        ui.same_line();
                        if ui.small_button("Aller") {
                            s.actions.push(Action::Load(i));
                        }
                        ui.same_line();
                        if ui.small_button("X") {
                            remove = Some(i);
                        }
                    }
                    if let Some(i) = remove {
                        s.slots.remove(i);
                        s.selected = s.selected.min(s.slots.len().saturating_sub(1));
                    }
                }

                if section(ui, "Sorties / points du niveau", false) {
                    if ui.button("Scanner le niveau") {
                        s.actions.push(Action::ScanPoints);
                    }
                    ui.text_disabled("Attention : aller sur une sortie peut finir le niveau.");
                    let here = s.pos;
                    let mut order: Vec<usize> = (0..s.points.len()).collect();
                    order.sort_by(|&a, &b| s.points[a].pos.dist(here).total_cmp(&s.points[b].pos.dist(here)));
                    ui.child_window("points").size([0.0, 160.0]).build(|| {
                        for i in order {
                            let _id = ui.push_id_usize(i);
                            if ui.small_button("Aller") {
                                s.actions.push(Action::Goto(s.points[i].pos));
                            }
                            ui.same_line();
                            ui.text(format!("{}  ({:.0} m)", s.points[i].label, s.points[i].pos.dist(here)));
                        }
                    });
                }

                if section(ui, &format!("Entites ({})", s.entities.len()), false) {
                    ui.checkbox(format!("Desactiver toutes les entites{}", hk(3)), &mut s.freeze_entities);
                    let here = s.pos;
                    for i in 0..s.entities.len() {
                        let _id = ui.push_id_usize(i);
                        let (id, pos, active) = (s.entities[i].id, s.entities[i].pos, s.entities[i].active);
                        if ui.small_button("Aller") {
                            s.actions.push(Action::Goto(pos));
                        }
                        ui.same_line();
                        if ui.small_button(if active { "Off" } else { "On" }) {
                            s.actions.push(Action::ToggleEntity(id));
                        }
                        ui.same_line();
                        ui.text(format!("{}  {:.0} m{}", s.entities[i].name, pos.dist(here), if active { "" } else { "  (off)" }));
                    }
                }

                if section(ui, &format!("Items ({})", s.items.len()), false) {
                    atomic_checkbox(ui, &format!("Items infinis (ne s'usent plus){}", hk(10)), &INF_ITEMS);
                    ui.input_text("Filtre", &mut self.item_filter).build();
                    let filter = self.item_filter.to_lowercase();
                    let mut give = None;
                    ui.child_window("items").size([0.0, 200.0]).build(|| {
                        for name in s.items.iter().filter(|n| n.to_lowercase().contains(&filter)) {
                            if ui.selectable(name) {
                                give = Some(name.clone());
                            }
                        }
                    });
                    if let Some(name) = give {
                        s.actions.push(Action::Give(name));
                    }
                }

                if let Some(p) = LOG.get() {
                    ui.separator();
                    ui.text_disabled(format!("Touches : {}", p.with_extension("cfg").display()));
                }
            });
    }

    fn message_filter(&self, _io: &Io) -> MessageFilter {
        if lock().menu_open {
            MessageFilter::InputAll
        } else {
            MessageFilter::empty()
        }
    }
}

unsafe fn start(hmodule: HINSTANCE) {
    let mut buf = [0u16; 1024];
    let n = GetModuleFileNameW(Some(HMODULE(hmodule.0)), &mut buf) as usize;
    let dll = PathBuf::from(String::from_utf16_lossy(&buf[..n]));
    _ = LOG.set(dll.with_extension("log"));
    _ = KEYS.set(config::load(&dll.with_extension("cfg")));
    log("--- injecte");

    let ready = (0..240).any(|_| {
        let ok = il2cpp::init();
        if !ok {
            std::thread::sleep(Duration::from_millis(500));
        }
        ok
    });
    if !ready {
        log("il2cpp introuvable, abandon");
        return;
    }
    // d3d12.dll is always loaded (this DLL imports it); D3D12Core.dll only once a real DX12 device exists.
    let dx12 = GetModuleHandleA(PCSTR(c"D3D12Core.dll".as_ptr() as _)).is_ok();
    log(&format!("rendu : {}", if dx12 { "DirectX 12" } else { "DirectX 11" }));

    // builder() initializes MinHook, which game::install also relies on.
    let overlay = Overlay { prev_keys: [false; config::ACTIONS.len()], item_filter: String::new() };
    let builder = Hudhook::builder();
    let builder = if dx12 { builder.with::<ImguiDx12Hooks>(overlay) } else { builder.with::<ImguiDx11Hooks>(overlay) };
    if let Err(e) = game::install() {
        log(&format!("erreur : {e}"));
        _ = INIT_ERROR.set(e);
    }
    if let Err(e) = builder.with_hmodule(hmodule).build().apply() {
        log(&format!("overlay : {e:?}"));
        return;
    }
    log("pret");
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
