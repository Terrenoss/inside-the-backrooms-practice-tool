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
anti-chute du jeu (5 s sans sol dessous), sommet de la bouée posée et options actives.

- **Joueur** : noclip, god mode, stamina / oxygène infinis, pas d'anxiété, vitesse de déplacement,
  hauteur de saut, désactiver le TP anti-chute.
- **Monde / visuel** : vitesse du jeu (0.1x – 2x, pour apprendre un timing au ralenti), effets VHS
  et post-process off, luminosité, débloquer tous les niveaux dans la sélection de niveau (rien n'est
  sauvegardé : ni fichier, ni statistique Steam ; utile en 0.7.0, qui garde les déblocages dans les
  stats Steam du compte). En 0.7.0, le bouton « Debloquer pour de bon tous les niveaux » (à confirmer,
  en pleine partie) fait pour chaque niveau ce que fait le jeu quand on l'atteint : sa statistique
  « REACH_… » passe à 1 dans tes stats Steam, et le niveau reste ouvert sans le tool.
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
  - en rose, la bouée posée reste visible à travers les murs, avec son « sommet » : la hauteur de
    son point le plus haut par rapport au sol sous tes pieds. Sur le toit, un sommet positif veut
    dire qu'elle dépasse du toit et que tu peux la viser. Le sommet s'affiche aussi dans le HUD.
  - un trait rose clair trace le trajet de la bouée pendant 5 s de jeu après le lâcher : on voit
    tout de suite si elle a été éjectée, repoussée par le toit ou si elle est retombée.
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
- **Positions (savestates)** : sauve position, rotation, angle vertical de la caméra, vie / stamina /
  oxygène / anxiété, effet de la gelée et inventaire. Une savestate faite en portant la bouée te la
  remet dans les mains au chargement (slot marqué « + bouee », « + gelee » sous la gelée).
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
