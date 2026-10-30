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

Marche avec les versions 0.7.35 (celle des records de speedrun) et 0.8.0 du jeu.

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
| F11 | Caméra libre (le joueur reste sur place, ZQSD / Espace / Ctrl / Shift déplacent la vue) |

Sans touche par défaut (assignables) : stamina infinie, items infinis, désactiver le TP anti-chute,
slot précédent, slot suivant, fins de niveau (`fins_niveau`).

Les touches se changent dans `libbackrooms_practice_tool.cfg`, créé à côté de la DLL au premier
lancement (lignes `action = TOUCHE`, vide = pas de touche). Noms possibles : F1-F12, A-Z, 0-9,
NUMPAD0-NUMPAD9, INSERT, DELETE, HOME, END, PAGEUP, PAGEDOWN. Relance le jeu et le lanceur après une
modification.

## Ce que contient le menu

**HUD** (toujours affiché) : position X/Y/Z, temps de la partie, temps passé en l'air avant le TP
anti-chute du jeu (5 s sans sol dessous) et options actives.

- **Joueur** : noclip, caméra libre, god mode, stamina / oxygène infinis, pas d'anxiété, vitesse de
  déplacement, hauteur de saut, désactiver le TP anti-chute.
  - Caméra libre : le joueur reste figé, sans tourner (sa capsule s'affiche en blanc). La souris et
    les touches du vol ne bougent que la vue, au moment de l'image. La bouée portée et l'aperçu du drop restent donc calculés depuis la vraie
    position du joueur.
- **Monde / visuel** : vitesse du jeu (0.1x – 2x, pour apprendre un timing au ralenti), effets VHS
  et post-process off, luminosité.
- **Hitbox / colliders** : affiche les vraies collisions autour de toi, avec la forme réelle des
  meshes et pas seulement des boîtes.
  - orange : mur invisible
  - bleu : collision du décor et des objets (option « + décor et objets »)
  - jaune : zone / trigger, avec son nom de près
  - rouge : TP, sortie, dégâts, changement de niveau
  - violet (option « Fins de niveau ») : tout ce qui termine le niveau, visible de n'importe où avec
    son nom. Ce sont les zones de sortie (y compris celles qui marchent au contact), la porte de
    fin du Fun Level (il faut FunDoorKey) et les objets de fin qu'on utilise avec E. Une zone de
    sortie se déclenche avec n'importe quel collider du joueur, pas seulement son corps.
  - Seul ce qui bloque vraiment le joueur est dessiné.
- **Bouée (Poolrooms)** : dans la technique du record (saopy, 38,5 s), on monte en lâchant la bouée
  (Q). Elle apparaît dans le personnage et le pousse vers le haut : on saute dessus et on la reprend
  aussitôt (E) : elle monte avec soi. On appuie de nouveau sur Q **pendant** la traversée du
  plafond : elle apparaît alors au niveau du toit et y reste, la corne au-dessus. Depuis le toit,
  on la vise et on la reprend (E). Lâchée depuis le haut de l'arche, sous le plafond, elle est
  éjectée et tombe.
  - Après chaque lâcher, son trajet est tracé en violet, et le HUD indique où elle a fini : tombée,
    restée sous le toit (à X cm) ou contre le toit (récupérable d'en haut).
  - « dans toi » dans l'étiquette : elle apparaîtra dans ton corps et te poussera.
  - La vraie forme de la bouée (corne comprise) est dessinée là où elle apparaîtra : en vert si elle
    dépassera du toit, en rouge si elle restera dessous. L'étiquette donne l'écart en cm.
  - Une fois lâchée, elle reste dessinée en violet à travers le toit pendant 60 s, pour viser.
  - Tant que tu la portes, elle reste attachée à toi et te suit partout, même à travers un plafond.
  Quand tu portes une bouée, une sphère montre où elle apparaîtra si tu la lâches maintenant (calcul
  fait par la fonction de drop du jeu), et un trait montre où elle tombe. Elle apparaît à hauteur
  de tes pieds + dropHeight, devant toi, sinon à droite, derrière ou à gauche. C'est l'orientation
  de ton corps qui compte, pas celle de la caméra, et l'étiquette indique le côté.
  - vert : elle reste à cette hauteur (pas forcément sur le toit)
  - orange : dans le décor, ou petite chute
  - rouge : elle retombe dans la salle
- **Glitch d'inspection** : inspecter un objet (inventaire, clic droit, Inspect) fait apparaître une
  copie solide de l'objet 10 cm devant la caméra, toujours orientée selon les axes du monde. Le jeu
  repousse alors le joueur hors de l'objet, parfois à travers une grille ou des rideaux. Plus
  l'objet est gros, plus la poussée est forte : la ProtectionSuit (bouton pour se la donner).
  - cyan : la forme réelle de l'objet, là où il apparaîtra si tu inspectes maintenant. Il faut
    l'avoir inspecté une fois pour que l'outil connaisse sa forme.
  - blanc : ta capsule de collision.
  - Après chaque inspection, le HUD et le jeu affichent la poussée mesurée (distance, direction
    avant/droite/haut). Vert si tu as traversé un mur, orange sinon.
- **Positions (savestates)** : sauve position, rotation, vie / stamina / oxygène / anxiété et inventaire. Une savestate faite en
  portant la bouée te la remet dans les mains au chargement (slot marqué « + bouee »).
- **Sorties / points du niveau** : scanne le niveau et liste sorties, portes et objets de fin, ascenseurs et triggers, avec
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
