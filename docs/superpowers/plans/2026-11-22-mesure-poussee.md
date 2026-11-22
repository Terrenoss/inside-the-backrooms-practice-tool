# Mesure complète de la poussée d'inspection — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remplacer la mesure simple actuelle (`Push` : départ → arrivée) par la mesure complète du §6.1. Elle donne :
- la poussée à l'ouverture, le maximum pendant l'inspection et le bilan après fermeture, en recul / monte / côté ;
- le mur derrière le joueur (enfoncé / épaisseur) et un verdict ;
- les dessins réglables et un historique des essais.

**Architecture:**
- **Logique pure** dans un nouveau module `tool/src/push.rs` : repère joueur, découpage d'un déplacement, verdict, géométrie du mur, enregistrement d'un essai. Elle est testée par `cargo test`.
- **`game.rs`** remplace `inspect_start` et `Push` par cet enregistrement. Il fait les lectures dans le jeu (capsule, caméra, raycasts) et les dessins.
- **`lib.rs`** affiche le HUD, les réglages et l'historique.

**Tech Stack:** Rust 2021, hudhook 0.9.3 (ImGui), API il2cpp résolue par nom.

**Spec:** `docs/superpowers/specs/2026-11-22-poussee-freecam-design.md` (§3 convention des angles, §4, §6.1, §6.7).

## Global Constraints

- **Texte des menus et du HUD en ASCII**, sans accents (« recul », « monte », « cote », « Effacer »).
- **Angles :**
  - « regard vers le bas » = `asin(-forward.y)` du `forward` monde du Transform de `Player.Camera`, en degrés ;
  - « orientation carte » = `atan2(avant.x, avant.z)` du repère joueur, en degrés.
- **Repère joueur :** construit depuis le vecteur **droite** de la caméra (horizontal, normalisé), avant = (−r.z, 0, r.x). Jamais depuis l'avant de la caméra.
- **Versions :** les nouveaux champs résolus par nom sont des `Option` (ici `<IsCrawling>k__BackingField` et `<IsCrouched>k__BackingField` de `BasePlayerController`). Absents → la pose s'affiche « ? ».
- **Pas de régression :** le bot de la bouée, la caméra libre et l'aperçu cyan marchent comme avant. `cargo test` et `selftest.py` restent verts.
- **Défauts :** bilan **1,5 s** après fermeture, fenêtre d'ouverture **0,15 s**, recherche du mur jusqu'à **1,5 m** derrière, historique limité à **30** lignes.

## Review Focus

1. **Regard droit vers le sol (90°) ou vers le ciel :** le repère reste correct (test `frame_from_right_survives_looking_down`).
2. **Pas de mur derrière** (rien à moins de 1,5 m) : verdict `PasDeMur`, pas de faux « traversé » (test `verdict_without_wall`).
3. **Inspection fermée avant la fin de la fenêtre d'ouverture (< 0,15 s), ou nouvelle inspection avant la fin du bilan :** l'essai en cours est fermé proprement (bilan pris à l'instant de la nouvelle ouverture) et un nouvel essai commence (test `new_open_finishes_previous_record`).
4. **Changement de niveau ou mort pendant un essai :** l'essai est abandonné sans panique. Le contrôleur change, donc `s.controller` change (vérifié à la main, Task 4).
5. **Caméra libre active :** la caméra et la position viennent toujours de la vraie tête, déjà garanti par `tick` qui tourne pendant `Update`. Les dessins restent sur le corps (vérifié à la main, Task 4).

---

### Task 1: Logique pure de la poussée (`push.rs`)

**Files:**
- Create: `tool/src/push.rs`
- Modify: `tool/src/lib.rs` (ajouter `mod push;` à côté de `mod game;`)
- Modify: `tool/src/game.rs`, `impl V3`. Rendre `add_scaled`, `sub`, `scale`, `dot`, `cross` `pub(crate)`. Ajouter `pub(crate) fn len(self) -> f32`.

**Interfaces:**
- Produces (dans `push.rs`, tout `pub(crate)`) :
  - `struct Frame { pub right: V3, pub fwd: V3 }`.
    - `Frame::from_camera_right(right: V3) -> Frame`. Repli `right = (1,0,0)` si la partie horizontale est < 1e-4.
    - `Frame::split(&self, d: V3) -> Split`.
    - `Frame::yaw_deg(&self) -> f32`.
  - `struct Split { pub back: f32, pub up: f32, pub side: f32 }`.
    - `back` = −(d·fwd), donc positif = recul. `up` = d.y. `side` = d·right, positif = droite.
    - `Split::len(&self) -> f32`.
  - `fn pitch_down_deg(cam_fwd: V3) -> f32` : `asin(clamp(-fwd.y))` en degrés.
  - `enum Pose { Debout, Accroupi, Allonge, Inconnue }`, `Pose::label(self) -> &'static str` : `"debout"`, `"accroupi"`, `"allonge"`, `"?"`.
  - `struct Wall { pub near: f32, pub far: f32 }` (distances depuis le centre de départ, vers l'arrière).
    - `Wall::thickness(&self) -> f32`.
    - `fn wall_from_hits(near: Option<f32>, back_hit: Option<f32>, probe: f32) -> Option<Wall>`.
      - `near` = distance du premier obstacle vers l'arrière.
      - `back_hit` = distance du premier obstacle en revenant depuis `near + probe`.
      - `far = near + probe - back_hit`.
      - `None` si `near` est `None`. Si `back_hit` est `None`, `far = near + probe`.
  - `enum Verdict { Traverse, Reste, Recrache, Monte, Rien, PasDeMur }`.
    - `Verdict::label(self) -> &'static str` : `"A TRAVERSE"`, `"reste en arriere"`, `"recrache"`, `"te fait monter"`, `"pas pousse"`, `"pas de mur derriere"`.
    - `Verdict::color(self) -> [f32; 4]` : vert `[0.2,1.0,0.3,1]`, jaune `[1.0,0.85,0.2,1]`, orange `[1.0,0.6,0.1,1]`, rouge `[1.0,0.25,0.25,1]`, gris `[0.7,0.7,0.7,1]`, gris clair `[0.85,0.85,0.85,1]`.
  - `fn verdict(open: Split, settled: Split, wall: Option<&Wall>, crossed: bool) -> Verdict`. Règles, dans l'ordre :
    1. `crossed`, ou `wall` présent et `settled.back > wall.far` → `Traverse`.
    2. `open.up > 0.1 && open.up > open.back` → `Monte`.
    3. `settled.back >= 0.1` → `Reste`, ou `PasDeMur` si `wall` est `None`.
    4. `open.back >= 0.05 || open.len() >= 0.05` → `Recrache`, ou `PasDeMur` si `wall` est `None`.
    5. Sinon `Rien`.
  - `struct PushRecord`, champs `pub` : `n: usize`, `item: String`, `yaw: f32`, `pitch: f32`, `pose: Pose`, `from: V3` (centre de la capsule avant), `radius: f32`, `height: f32`, `frame: Frame`, `opened: Instant`, `closed: Option<Instant>`, `open: Split`, `max: f32`, `settled: Option<Split>`, `wall: Option<Wall>`, `crossed: bool`, `trail: Vec<V3>`, `start: Option<Saved>` (slot de départ).
    - `PushRecord::depth(&self, s: Split) -> Option<f32>` : `s.back + radius - wall.near`, soit l'enfoncement de l'arrière de la capsule dans le mur.
    - `PushRecord::verdict(&self) -> Option<Verdict>` : `None` tant que `settled` est `None`.
    - `PushRecord::hud(&self) -> String` : texte d'une ligne (Task 3).
  - `Saved` doit être `pub(crate)` dans `game.rs` (c'est déjà `pub struct Saved`).

- [ ] **Step 1: Write the failing tests** (module `#[cfg(test)] mod tests` dans `push.rs`)

```rust
#[test]
fn frame_from_right_survives_looking_down() {
    // regard droit vers le sol, tourne vers +X : la droite de la camera vaut (0,0,-1)
    let f = Frame::from_camera_right(V3 { x: 0.0, y: 0.0, z: -1.0 });
    assert!(f.fwd.dist(V3 { x: 1.0, y: 0.0, z: 0.0 }) < 1e-5);
    assert!((f.yaw_deg() - 90.0).abs() < 1e-3);
    let s = f.split(V3 { x: -0.27, y: 0.0, z: 0.0 });
    assert!((s.back - 0.27).abs() < 1e-5 && s.up.abs() < 1e-6 && s.side.abs() < 1e-6);
}

#[test]
fn pitch_convention() {
    assert!((pitch_down_deg(V3 { x: 0.0, y: -1.0, z: 0.0 }) - 90.0).abs() < 1e-3);
    let f = V3 { x: 0.0, y: -(62f32.to_radians().sin()), z: 62f32.to_radians().cos() };
    assert!((pitch_down_deg(f) - 62.0).abs() < 1e-3);
}

#[test]
fn wall_geometry() {
    let w = wall_from_hits(Some(0.3), Some(2.6), 3.0).unwrap();
    assert!((w.far - 0.7).abs() < 1e-5 && (w.thickness() - 0.4).abs() < 1e-5);
    assert!(wall_from_hits(None, Some(1.0), 3.0).is_none());
    assert!((wall_from_hits(Some(0.3), None, 3.0).unwrap().far - 3.3).abs() < 1e-5);
}

#[test]
fn verdict_cases_from_the_logs() {
    let sp = |back, up| Split { back, up, side: 0.0 };
    let wall = Wall { near: 0.3, far: 0.9 };
    // mur 2 a 62 deg : recul 0,27 qui reste
    assert_eq!(verdict(sp(0.27, 0.0), sp(0.27, 0.0), Some(&wall), false), Verdict::Reste);
    // mur 2 a 76-80 deg : monte de 0,27
    assert_eq!(verdict(sp(0.0, 0.27), sp(0.0, 0.0), Some(&wall), false), Verdict::Monte);
    // mur 2 a 47 deg : recul 0,25 puis recrache
    assert_eq!(verdict(sp(0.25, 0.0), sp(0.0, 0.0), Some(&wall), false), Verdict::Recrache);
    // passe la face arriere
    assert_eq!(verdict(sp(0.37, 0.0), sp(1.0, 0.0), Some(&wall), false), Verdict::Traverse);
    assert_eq!(verdict(sp(0.0, 0.0), sp(0.0, 0.0), Some(&wall), false), Verdict::Rien);
}

#[test]
fn verdict_without_wall() {
    let sp = |back| Split { back, up: 0.0, side: 0.0 };
    assert_eq!(verdict(sp(0.3), sp(0.3), None, false), Verdict::PasDeMur);
    assert_eq!(verdict(sp(0.3), sp(0.3), None, true), Verdict::Traverse);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p backrooms_practice_tool push::`
Expected: compile errors, because `Frame`, `verdict`… are not defined yet.

- [ ] **Step 3: Implement everything in the Interfaces block in `tool/src/push.rs`**, plus the `pub(crate)` changes in `impl V3`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p backrooms_practice_tool`
Expected: all tests PASS, the 16 existing ones and the 5 new ones.

- [ ] **Step 5: Commit** — `git commit -m "Push measurement: pure logic (player frame, split, wall, verdict)"`

---

### Task 2: Capture dans le jeu (ouverture, pendant, bilan, mur)

**Files:**
- Modify: `tool/src/game.rs`
  - `track_inspection` (≈ l. 2365).
  - `State`, champs `push`, `inspect_start` (≈ l. 677, 744).
  - Le bloc `Action::Save` (≈ l. 2795), extrait en `snapshot`.
  - `G` et `resolve`, pour les offsets de pose.
  - Supprimer `struct Push` / `impl Push` (≈ l. 510-560) et `PUSH_SHOWN` (le remplacer par `push_shown` dans `State`).

**Interfaces:**
- Consumes : tout `push::*` de la Task 1. Fonctions existantes : `capsule(g, cc) -> (V3, f32, f32)`, `ray(g, from, dir, max, mask) -> Option<f32>`, `collision_mask(g, layer, &[i32])`, `blocked(g, from, to, layer) -> bool`, `INSPECT_LAYER`.
- Produces :
  - `State` gagne, en `pub` :
    - `push: Option<PushRecord>` : l'essai en cours ou le dernier ;
    - `push_log: Vec<PushRecord>` : historique, le plus récent à la fin, tronqué à `push_log_max` ;
    - `push_log_max: usize` = 30, `push_settle: f32` = 1.5 (secondes), `push_wall_probe: f32` = 1.5 (mètres) ;
    - `push_shown: f32` = 15.0 (secondes d'affichage du dernier essai dans le monde et le HUD) ;
    - `push_n: usize` (compteur).
  - `G` gagne `off_pose: Option<(usize, usize)>`, soit (IsCrawling, IsCrouched) sur `bpc`.
  - `unsafe fn snapshot(g: &G, s: &State, ctrl: P, tr: P, stats: P, inv: P, carried: Option<P>) -> Saved`, utilisé par `Action::Save` et par l'ouverture d'un essai.

- [ ] **Step 1: Write the failing test** (dans `push.rs`, logique de cycle de vie qu'on ajoute en pur)
  - `PushRecord::finish(&mut self, now_center: V3, closed: Instant)` : met `settled = Some(frame.split(now_center - from))` et `closed = Some(...)` si ce n'est pas déjà fait.
  - Pour tester sans jeu, `PushRecord::new_for_test(from: V3, frame: Frame, wall: Option<Wall>) -> PushRecord`, en `#[cfg(test)]`, qui remplit le reste avec des valeurs neutres.

```rust
#[test]
fn new_open_finishes_previous_record() {
    let f = Frame::from_camera_right(V3 { x: 1.0, y: 0.0, z: 0.0 }); // avant = +Z
    let mut r = PushRecord::new_for_test(V3::default(), f, Some(Wall { near: 0.3, far: 0.9 }));
    r.finish(V3 { x: 0.0, y: 0.0, z: -0.27 }, Instant::now());
    assert!((r.settled.unwrap().back - 0.27).abs() < 1e-5);
    // un second finish (bilan deja pris) ne change rien
    r.finish(V3::default(), Instant::now());
    assert!((r.settled.unwrap().back - 0.27).abs() < 1e-5);
    assert!(r.depth(r.settled.unwrap()).unwrap() > 0.0 || r.radius == 0.0);
}
```

- [ ] **Step 2: Run it to see it fail**

Run: `cargo test -p backrooms_practice_tool push::tests::new_open_finishes_previous_record`
Expected: FAIL, `finish` / `new_for_test` are not defined.

- [ ] **Step 3: Implement `finish`, `new_for_test` in `push.rs`.** Then rewrite `track_inspection(g, ins, s, cc, before, ctrl, tr, stats, inv, carried)` in `game.rs` (the `tick` call is updated to match):
  - **Ouverture** (`is_inspecting` passe à vrai) :
    - si `s.push` a un essai non fermé, `finish` au centre actuel ;
    - lire caméra (`Player.Camera`) : `forward`, `right` → `Frame::from_camera_right(right)`, `pitch_down_deg(forward)` ;
    - pose via `off_pose` ;
    - `from = before.map_or(center, |c| c.0)` (comme aujourd'hui) ;
    - copie `inspect_item` / `inspect_shapes` (comme aujourd'hui) ;
    - **mur :** `near = ray(g, from, -fwd, push_wall_probe, mask)` puis `back_hit = ray(g, from - fwd*(near+3.0), fwd, 3.0, mask)`, avec `mask = collision_mask(g, layer, &[layer, INSPECT_LAYER])`, puis `wall_from_hits(near, back_hit, 3.0)` ;
    - `start = Some(snapshot(...))` ;
    - pousser un nouveau `PushRecord` dans `s.push` (`n = push_n += 1`).
  - **Chaque frame tant que l'essai n'a pas de `settled` :**
    - si `opened.elapsed() <= 0.15 s` : `open` = split du déplacement depuis `from` ;
    - `max = max(max, dist)` ;
    - `trail.push(center)` si déplacé de plus de 1 cm depuis le dernier point.
  - **Fermeture** (`is_inspecting` repasse à faux) : `closed = Some(now)`, et `crossed = blocked(g, from, center, layer)` pris à chaque frame jusqu'au bilan (OU logique).
  - **Bilan :** quand `closed.elapsed() >= push_settle`, `finish(center, ...)`, puis copier l'essai dans `push_log` (tronquer au début si `len > push_log_max`).
  - **Changement de contrôleur** (nouveau niveau, mort) : `s.push = None` sans l'ajouter à l'historique.

- [ ] **Step 4: Run tests and build**

Run: `cargo test -p backrooms_practice_tool` then `cargo build --release --target-dir target/essai`. If the DLL is locked, rename it to `.old-HHMMSS` first.
Expected: tests PASS, build OK, `python -I selftest.py "<jeu>" "<copie de la dll>"` → `SELFTEST OK`.

- [ ] **Step 5: Commit** — `git commit -m "Push measurement: capture open / max / settle, wall behind, start slot"`

---

### Task 3: Dessins et HUD

**Files:**
- Modify: `tool/src/game.rs`, `draw_inspect` (≈ l. 2400-2455) and `draw` (condition `inspect`, ≈ l. 2023)
- Modify: `tool/src/push.rs`, `PushRecord::hud`

**Interfaces:**
- Consumes : `s.push`, `push_shown`.
- Produces : `State` gagne des `pub bool`, tous à `true` par défaut sauf `show_push_copy` :
  - `show_push_ghost` : capsule fantôme au départ ;
  - `show_push_body` : capsule actuelle ou au bilan, couleur du verdict ;
  - `show_push_arrow` : trait départ → maintenant ;
  - `show_push_trail` : le trajet ;
  - `show_push_wall` : face avant et face arrière du mur, deux carrés de 0,5 m perpendiculaires à `fwd`, centrés sur `from - fwd*near` et `from - fwd*far`, en gris, avec une étiquette `"mur : epaisseur X cm"` ;
  - `show_push_copy` : la copie réelle pendant l'inspection, dessinée au vrai pivot avec `item_shapes`.

- [ ] **Step 1: Write the failing test** pour `PushRecord::hud`.
  - Format exact pendant le bilan : `"#3 62 deg allonge | ouverture : recul 0.27 monte 0.00 cote +0.00 | bilan..."`.
  - Puis, une fois le bilan pris : `"#3 62 deg allonge | ouverture : recul 0.27 monte 0.00 cote +0.00 | bilan : recul 0.27 | dans le mur 12 cm / ep. 40 cm | reste en arriere"`.
  - Sans mur : pas de partie « dans le mur ».

```rust
#[test]
fn hud_line() {
    let f = Frame::from_camera_right(V3 { x: 1.0, y: 0.0, z: 0.0 });
    let mut r = PushRecord::new_for_test(V3::default(), f, Some(Wall { near: 0.45, far: 0.85 }));
    (r.n, r.pitch, r.pose, r.radius) = (3, 62.0, Pose::Allonge, 0.3);
    r.open = Split { back: 0.27, up: 0.0, side: 0.0 };
    assert!(r.hud().ends_with("| bilan..."));
    r.finish(V3 { x: 0.0, y: 0.0, z: -0.27 }, Instant::now());
    assert_eq!(r.hud(), "#3 62 deg allonge | ouverture : recul 0.27 monte 0.00 cote +0.00 | bilan : recul 0.27 | dans le mur 12 cm / ep. 40 cm | reste en arriere");
}
```

- [ ] **Step 2: Run it** — `cargo test -p backrooms_practice_tool push::tests::hud_line`. Expected: FAIL.
- [ ] **Step 3: Implement `hud()`, then the drawing in `draw_inspect` and the `draw` condition.**
  - Dessiner tant que l'essai n'a pas de `settled`, ou dans les `push_shown` secondes après `closed`.
  - L'étiquette du corps est `hud()` complet, de la couleur du verdict (blanc tant qu'il n'y en a pas).
- [ ] **Step 4: Run tests + release build + selftest** (même commande qu'à la Task 2). Expected: PASS / `SELFTEST OK`.
- [ ] **Step 5: Commit** — `git commit -m "Push measurement: ghost, body, arrow, trail, wall faces, copy; HUD line"`

---

### Task 4: Menu, historique, README, test en jeu

**Files:**
- Modify: `tool/src/lib.rs`
  - HUD : remplacer la ligne `"Inspection : ..."` par `s.push.hud()` tant que c'est récent (≈ l. 183).
  - Section `"Glitch d'inspection"` (≈ l. 387).
- Modify: `README.md`, section « Glitch d'inspection ».

**Interfaces:**
- Consumes : tout ce qui précède. `Action::LoadAt(Saved)` existe. Pour ajouter un slot : `s.slots.push(saved)`.

- [ ] **Step 1: Menu.** Dans « Glitch d'inspection », sous les réglages existants :
  - cases `"Capsule de depart"`, `"Capsule apres"`, `"Fleche"`, `"Trajet"`, `"Faces du mur"`, `"Copie reelle pendant l'inspection"` ;
  - curseurs `"Bilan apres (s)"` 0.5–5.0, `"Chercher le mur jusqu'a (m)"` 0.5–3.0, `"Lignes d'historique"` 5–100 ;
  - puis `"Historique"` : un `child_window` de 200 px.
    - Une ligne par essai, du plus récent au plus ancien, en couleur du verdict : `"#n  objet  carte Y deg  regard P deg  pose | ouverture recul/monte | bilan recul | verdict"`.
    - Deux petits boutons par ligne : `"Slot"` (ajoute `start` aux slots) et `"Aller"` (`Action::LoadAt(start)`).
    - Bouton `"Effacer"` qui vide `push_log`.
- [ ] **Step 2: README.** Décrire en 6 à 10 lignes les nouvelles mesures, les couleurs des verdicts, l'historique et ses boutons, et la convention d'angle.
- [ ] **Step 3: Build + tests + selftest.** Expected: PASS / `SELFTEST OK`.
- [ ] **Step 4: Commit** — `git commit -m "Push measurement: menu settings, history with slot buttons, README"`
- [ ] **Step 5: Test en jeu, par le joueur.** Lui donner la liste :
  - mur 1 à 80° : recul ≈ 0,30, « reste en arriere » ;
  - mur 2 à 62° : recul ≈ 0,27, « reste en arriere » ;
  - mur 2 à 76°+ : « te fait monter » ;
  - mur 2 à 47° : « recrache » ;
  - la 2e inspection au mur 1 : « A TRAVERSE » ;
  - faire un essai en caméra libre ;
  - changer de niveau pendant un bilan : pas de plantage.

  Comparer avec `speedrun_tools/Lancer_journal.bat` si un chiffre semble faux.
