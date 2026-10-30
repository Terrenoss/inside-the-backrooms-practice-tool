# Inside the Backrooms — Practice Tool

Outil d'entraînement pour le speedrun d'**Inside the Backrooms**, dans l'esprit des practice tools de
jdsd (Dark Souls III / Elden Ring) : savestates, noclip, affichage des hitbox et des triggers,
téléportation vers les sorties, aide pour le glitch de la bouée des Poolrooms…

Un menu s'affiche par-dessus le jeu (overlay ImGui). Le tool est injecté dans le jeu par un petit
lanceur, sans aucun fichier du jeu modifié.

> ⚠️ **Pour l'entraînement uniquement.** Ne l'utilise pas pendant une run que tu soumets.
> Fait pour le solo (tu es l'hôte de la partie).

## Installation

1. Télécharge le zip de la dernière [release](../../releases/latest) et extrais-le où tu veux.
   `backrooms_practice_tool.exe` et `libbackrooms_practice_tool.dll` doivent rester dans le même dossier.
2. Lance **Inside the Backrooms**.
3. Lance `backrooms_practice_tool.exe`. Il affiche « Practice tool injecte ».
4. En jeu, appuie sur **F1** pour ouvrir le menu.

À refaire à chaque lancement du jeu.

## Touches

| Touche | Action |
|--------|--------|
| F1 | Ouvrir / fermer le menu |
| F2 | Noclip / vol (ZQSD, Espace monte, Ctrl descend, Shift x3) |
| F3 | God mode |
| F4 | Désactiver toutes les entités |
| F5 | Savestate : sauver |
| F6 | Savestate : charger le slot sélectionné |
| F7 | Effets VHS / caméra off |
| F8 | Afficher les murs invisibles |
| F9 | Afficher les zones / triggers |

Sans touche par défaut (assignables) : stamina infinie, items infinis, désactiver le TP anti-chute,
slot précédent, slot suivant.

Les touches se changent dans `libbackrooms_practice_tool.cfg`, créé à côté de la DLL au premier
lancement (lignes `action = TOUCHE`, vide = pas de touche). Noms possibles : F1-F12, A-Z, 0-9,
NUMPAD0-NUMPAD9, INSERT, DELETE, HOME, END, PAGEUP, PAGEDOWN. Relance le jeu et le lanceur après une
modification.

## Ce que contient le menu

**HUD** (toujours affiché) : position X/Y/Z, temps de la partie, temps passé en l'air avant le TP
anti-chute du jeu (5 s sans sol dessous) et options actives.

- **Joueur** : noclip, god mode, stamina / oxygène infinis, pas d'anxiété, vitesse de déplacement,
  hauteur de saut, désactiver le TP anti-chute.
- **Monde / visuel** : vitesse du jeu (0.1x – 2x, pour apprendre un timing au ralenti), effets VHS
  et post-process off, luminosité.
- **Hitbox / colliders** : affiche les vraies collisions autour de toi, avec la forme réelle des
  meshes et pas seulement des boîtes.
  - orange : mur invisible
  - bleu : collision du décor et des objets (option « + décor et objets »)
  - jaune : zone / trigger, avec son nom de près
  - rouge : TP, sortie, dégâts, changement de niveau
  - Seul ce qui bloque vraiment le joueur est dessiné.
- **Bouée (Poolrooms)** : quand tu portes une bouée, une sphère montre où elle apparaîtra si tu la
  lâches maintenant (calcul fait par la fonction de drop du jeu), et un trait montre où elle tombe.
  - vert : elle reste à cette hauteur (sur le toit)
  - orange : dans le décor, ou petite chute
  - rouge : elle retombe dans la salle
- **Positions (savestates)** : sauve position, rotation, vie / stamina / oxygène / anxiété et inventaire. Une savestate faite en
  portant la bouée te la remet dans les mains au chargement (slot marqué « + bouee »).
- **Sorties / points du niveau** : scanne le niveau et liste sorties, ascenseurs et triggers, avec
  un bouton pour s'y téléporter. Attention : aller sur une sortie peut finir le niveau.
- **Entités** : liste des entités avec leur distance, téléportation, on/off une par une.
- **Items** : donne n'importe quel item, items infinis.

## Problèmes

- **« Jeu introuvable »** : lance le jeu avant le lanceur.
- **L'antivirus bloque le lanceur** : l'injection de DLL est souvent signalée à tort. Ajoute une
  exception pour le dossier, ou compile toi-même depuis les sources.
- **Le menu affiche une erreur** : une mise à jour du jeu a peut-être renommé quelque chose.
  Regarde `libbackrooms_practice_tool.log` à côté de la DLL et ouvre une issue avec son contenu.

## Compiler depuis les sources

Il faut [Rust](https://rustup.rs) (toolchain MSVC, Windows x64).

```
cargo build --release
```

Les fichiers sont dans `target/release/` : `backrooms_practice_tool.exe` et
`libbackrooms_practice_tool.dll`.

`cargo test` lance les tests unitaires. `selftest.py` démarre le runtime du jeu sans l'ouvrir et
vérifie que toutes les méthodes et tous les champs utilisés par le tool existent encore :

```
python -I selftest.py "<dossier du jeu>" "target/release/libbackrooms_practice_tool.dll"
```

## Comment ça marche

Le jeu est en Unity IL2CPP. La DLL (Rust + [hudhook](https://github.com/veeenu/hudhook)) retrouve les
classes et méthodes du jeu par leur nom au démarrage via l'API il2cpp, au lieu d'adresses fixes. Elle
continue donc à marcher après une mise à jour tant que les noms ne changent pas.
