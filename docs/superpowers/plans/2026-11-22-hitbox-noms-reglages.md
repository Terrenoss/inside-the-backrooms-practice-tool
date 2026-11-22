# Hitbox nommees et reglages separes — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Spec §6.5 (chaque hitbox dessinee peut dire ce qu'elle est) et §6.6 (plus de reglage qui en active plusieurs en douce).

**Architecture:** tout dans `tool/src/game.rs` (State, describe, draw_colliders, visuals) et `tool/src/lib.rs` (menu, HUD, touches). Les decisions pures sont des fonctions testees par `cargo test`.

**Tech Stack:** Rust 2021, hudhook 0.9.3.

**Spec:** `docs/superpowers/specs/2026-11-22-poussee-freecam-design.md` §6.5, §6.6.

## Global Constraints
- Texte des menus en ASCII, sans accents.
- F7 coupe exactement les 10 effets de `off_fx` et le VHS du jeu, comme avant. On n'ajoute ni balance des blancs ni `colorAdjustments` (le curseur de luminosite ecrit dedans).
- F8 = murs invisibles. F9 = zones ET speciaux ensemble, comme avant.
- `cargo test` et `selftest.py` restent verts.

## Review Focus
1. **Beaucoup de colliders autour du joueur** (Poolrooms) : les noms de decor sont desactives par defaut, et la visee ne lance un rayon que sur les colliders dessines dans le rayon d'affichage.
2. **Ancienne valeur de F7 :** il n'y a plus de `fx_off`. F7 coupe tout si au moins un effet tourne, sinon il remet tout (test `f7_cuts_all_then_restores`).
3. **Exits sans collider** (marqueurs) : pas de forme ni de couche dans l'etiquette, et aucun plantage.

---

### Task 1: Decisions pures + etat
**Files:** `tool/src/game.rs` (State, `Kind`, tests).
**Produces :**
- `State` :
  - `fx_cut: [bool; 10]`, `fx_game_vhs: bool` : remplacent `fx_off` ;
  - `hitbox_unwarp: bool` (= true) ;
  - `show_specials: bool` ;
  - `label_invisible`, `label_solid`, `label_trigger`, `label_special`, `label_exit` : (true, false, true, true, true) ;
  - `label_dist: f32` (= 15) ;
  - `label_shape: bool` ;
  - `label_aimed: bool` (= true) ;
  - `label_only_aimed: bool` ;
  - `pub aimed: Option<(String, [f32; 4])>`.
- `pub const FX_NAMES: [&str; 10]`.
- Methodes `State::toggle_fx_all(&mut self)`, `State::fx_all_cut(&self) -> bool`, `State::toggle_zones(&mut self)`, `State::scans_colliders(&self) -> bool`, `State::any_hitboxes(&self) -> bool`.
- `fn kind_wanted(kind: Kind, s: &State) -> bool`, `fn label_wanted(kind: Kind, s: &State) -> bool`.
- `fn collider_label(kind: Kind, label: &str, name: &str, shape: &str, layer: i32, with_shape: bool) -> String` :
  - texte de base : `mur invisible : {name}`, `decor : {name}`, `zone : {name}` ; `label` tel quel pour Special et Exit ;
  - suffixe ` [{shape}, couche {layer}]` si `with_shape` et que `shape` n'est pas vide.

- [ ] Tests d'abord :
  - `f7_cuts_all_then_restores` ;
  - `f9_toggles_zones_and_specials_together` ;
  - `decor_drawn_without_invisible_walls` ;
  - `collider_labels`.
  Les voir echouer, puis implementer et voir la suite passer. Commit.

### Task 2: Branchement dans le jeu
**Files:** `tool/src/game.rs`.
- `Info` gagne `name: String`, `shape_kind: &'static str` (« Box », « Sphere », « Capsule », « Mesh », « Terrain », sinon « autre ») et `layer: i32`. Ils sont remplis dans `describe` ; pour les fins de niveau, `name_of`, la forme si c'est un collider et la couche.
- `draw_colliders` utilise `kind_wanted`, `label_wanted`, `label_dist` et `collider_label`.
  - Il prend un `aim: Ray` (oeil + avant de la camera rendue).
  - Si `label_aimed` : il lance `Collider.Raycast` sur chaque collider dessine, prend le plus proche dans `collider_radius`, met son etiquette au point touche et la range dans `s.aimed`.
  - `label_only_aimed` coupe les autres etiquettes.
- `scan_colliders`, `draw` et `visuals` utilisent `scans_colliders` / `any_hitboxes`.
- `visuals` utilise `fx_cut[i] || (hitbox_unwarp && any_hitboxes && WARPING)` et `fx_game_vhs`.
- [ ] `cargo test`, build release, `selftest.py`. Commit.

### Task 3: Menu, HUD, touches, README
**Files:** `tool/src/lib.rs`, `README.md`.
- Touches :
  - F7 → `toggle_fx_all` ;
  - F9 → `toggle_zones`.
- Monde / visuel :
  - bouton `Tout couper [F7]` / `Tout remettre` ;
  - case par effet (`Couper : {FX_NAMES[i]}`) et `Couper : VHS du jeu`.
- Hitbox :
  - cases `Murs invisibles [F8]`, `Decor et objets`, `Zones [F9]`, `TP / sorties / degats [F9]`, `Fins de niveau` ;
  - noms par type ;
  - `Distance des noms (m)` ;
  - `+ forme et couche` ;
  - `Nom de la hitbox visee (viseur)` ;
  - `Seulement celle visee` ;
  - `Couper panini / VHS / distorsion pendant les hitbox`.
- Glitch : retirer les 2 anciennes lignes de legende (vert / orange).
- HUD : `Vise : {aimed}`.
- README : les touches et les sections Monde / Hitbox.
- [ ] Build, tests, selftest. Commit.
