# Inside the Backrooms — Practice Tool

Outil d'entraînement pour le speedrun d'**Inside the Backrooms**, dans l'esprit des practice tools de
jdsd (Dark Souls III / Elden Ring) : savestates, noclip, affichage des hitbox et des triggers,
téléportation vers les sorties, aide pour le glitch de la bouée des Poolrooms…

Un menu s'affiche par-dessus le jeu (overlay ImGui). Le tool est injecté dans le jeu par un petit
lanceur, sans aucun fichier du jeu modifié.

> ⚠️ **Pour l'entraînement uniquement.** Ne l'utilise pas pendant une run que tu soumets.
> Fait pour le solo (tu es l'hôte de la partie).

Marche sur les versions 0.8.0, 0.7.35 et 0.7.0 du jeu. Avant la 0.8.0, la liste des sorties n'a pas
les débuts / fins de niveau (le jeu ne les a pas encore à ces versions).

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
| F7 | Couper / remettre tous les effets de caméra (VHS, flou, grain…) |
| F8 | Afficher / cacher les murs invisibles (et le décor s'il était affiché) |
| F9 | Afficher les zones et les TP / sorties / dégâts (ensemble) |
| F11 | Caméra libre (le joueur reste sur place, ZQSD / Espace / Ctrl / Shift déplacent la vue) |

Sans touche par défaut (assignables) : stamina infinie, items infinis, désactiver le TP anti-chute,
slot précédent, slot suivant, fins de niveau (`fins_niveau`).

Les touches se changent dans `libbackrooms_practice_tool.cfg`, créé à côté de la DLL au premier
lancement (lignes `action = TOUCHE`, vide = pas de touche). Noms possibles : F1-F12, A-Z, 0-9,
NUMPAD0-NUMPAD9, INSERT, DELETE, HOME, END, PAGEUP, PAGEDOWN. Relance le jeu et le lanceur après une
modification.

## Ce que contient le menu

**HUD** (toujours affiché) : position X/Y/Z, temps de la partie, temps passé en l'air avant le TP
anti-chute du jeu (5 s sans sol dessous), sommet de la bouée posée et options actives.

- **Joueur** : noclip, caméra libre, god mode, stamina / oxygène infinis, pas d'anxiété, vitesse de
  déplacement, hauteur de saut, désactiver le TP anti-chute.
  - Caméra libre : la souris et les touches du vol (ZQSD, Shift, Espace) ne bougent que la vue,
    au moment de l'image. Le perso ne marche plus, mais le jeu le bouge encore (poussée, gravité,
    s'allonger). Sa capsule s'affiche en blanc. La bouée portée, l'aperçu du drop et l'aperçu
    d'inspection restent calculés depuis sa vraie tête.
- **Monde / visuel** : vitesse du jeu (0.1x – 2x, pour apprendre un timing au ralenti), effets de
  caméra (bouton « Tout couper » / F7, ou une case par effet : vignette, aberration, flou de mouvement,
  panini, grain, flou de profondeur, vision trouble, basse résolution, VHS de la caméra, distorsion,
  VHS du jeu), post-process du niveau off, luminosité, débloquer tous les niveaux dans la sélection de niveau (rien n'est
  sauvegardé : ni fichier, ni statistique Steam ; utile en 0.7.0, qui garde les déblocages dans les
  stats Steam du compte). En 0.7.0, le bouton « Debloquer pour de bon tous les niveaux » (à confirmer,
  en pleine partie) fait pour chaque niveau ce que fait le jeu quand on l'atteint : sa statistique
  « REACH_… » passe à 1 dans tes stats Steam, et le niveau reste ouvert sans le tool.
- **Hitbox / colliders** : affiche les vraies collisions autour de toi, avec la forme réelle des
  meshes et pas seulement des boîtes.
  - Une case par type, indépendante des autres :
    - orange : mur invisible ;
    - bleu : collision du décor et des objets ;
    - jaune : zone ;
    - rouge : TP, sortie, dégâts, changement de niveau.
  - **Noms :** une case par type pour afficher le nom à côté de chaque hitbox, avec ce qu'elle est
    (« mur invisible : BoundsCollider »).
    - Distance réglable (15 m par défaut).
    - « + forme et couche » ajoute par exemple « [Mesh, couche 0] ».
    - « Nom de la hitbox visee » affiche le nom de celle sous le viseur (aussi dans le HUD, « Vise : … »).
    - « Seulement celle visee » cache les autres noms.
  - « Couper panini / VHS / distorsion pendant les hitbox » (active par défaut) : ces effets
    déforment l'image et décalent les traits.
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
  - Cases « Voir la bouee a travers les murs » et « Trace apres le lacher » : la bouée lâchée
    et son trajet.
  - **test en boucle (un bot qui apprend)** : à chaque essai, il recharge le slot sélectionné (fait
    en portant la bouée), tourne la tête jusqu'au cercle vert, lâche la bouée, puis une fois sur le
    toit la vise et fait E si elle est sous le curseur et à portée. Réussi = sur le toit avec la
    bouée en main. Il tourne jusqu'à « Arreter ».
    - Avec un deuxième slot (« Jusqu'au slot »), il essaie un endroit tous les 5 cm entre les deux :
      fais un slot à chaque bout de l'arche, collé devant.
    - Il apprend quelle combinaison endroit + angle de tête (de -60 à +60°, par pas de 3°) marche :
      il essaie chaque angle qui donne un cercle vert, puis revient plus souvent sur ce qui réussit.
      Le meilleur line up est celui qui marche encore à 5 cm ou 3° près.
    - Option : ignorer les cercles verts à ta droite (la bouée y tombe). Le menu donne le taux de
      réussite selon le côté du cercle vert.
    - Le line up s'affiche au sol : rond blanc où te mettre, flèche où regarder, croix où viser la
      corne ; le HUD dit de combien tourner. Un bouton le sauve comme slot pour t'entraîner dessus.
- **Glitch d'inspection** : inspecter un objet (inventaire, clic droit, Inspect) fait apparaître une
  copie solide de l'objet 10 cm devant la caméra, toujours orientée selon les axes du monde. Le jeu
  repousse alors le joueur hors de l'objet, parfois à travers une grille ou des rideaux. Plus
  l'objet est gros, plus la poussée est forte : la ProtectionSuit (bouton pour se la donner).
  - cyan : la forme réelle de l'objet, là où il apparaîtra si tu inspectes maintenant. Il faut
    l'avoir inspecté une fois pour que l'outil connaisse sa forme.
  - blanc : ta capsule de collision.
  - Curseurs « Remplissage de l'objet » et « Epaisseur des traits » pour mieux voir la forme.
  - **Mesure de la poussée.** À chaque inspection, le HUD affiche une ligne du type
    `#3 62 deg allonge | ouverture : recul 0.27 monte 0.00 cote +0.00 | bilan : recul 0.27 | dans le mur 12 cm / ep. 40 cm | reste en arriere`.
    - « regard » : degrés sous l'horizon (90 = le sol).
    - « ouverture » : la poussée dans les 0,15 s après l'ouverture.
    - « bilan » : où tu es 1,5 s après la fermeture (réglable). Le mur peut te recracher entre-temps.
    - Le mur derrière toi est cherché jusqu'à 1,5 m (réglable). Le tool donne son épaisseur et de
      combien ta capsule est dedans.
  - **Verdict :**
    - vert : « A TRAVERSE » ;
    - jaune : « reste en arriere » ;
    - orange : « recrache » ;
    - rouge : « te fait monter » ;
    - gris : « pas pousse » ou « pas de mur derriere ».
  - **Dessins, chacun avec sa case :** capsule de départ (gris), capsule après (couleur du
    verdict), flèche, trajet du corps, faces du mur, copie réelle de l'objet pendant l'inspection.
  - **Historique des essais :** numéro, objet, orientation, regard, pose, chiffres, verdict.
    - « Slot » sauve la position de départ de l'essai comme savestate.
    - « Aller » t'y remet.
    - « Effacer » vide la liste.
  - **Caméra libre (F11) :** ZQSD / Shift / Espace bougent seulement la caméra. Le perso reste
    soumis au jeu (poussée, gravité, s'allonger), et l'aperçu reste devant sa vraie tête.
- **Positions (savestates)** : sauve position, rotation, angle vertical de la caméra, vie / stamina /
  oxygène / anxiété, effet de la gelée et inventaire. Une savestate faite en portant la bouée te la
  remet dans les mains au chargement (slot marqué « + bouee », « + gelee » sous la gelée).
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
