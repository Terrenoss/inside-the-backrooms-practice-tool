# Bot bouée (Poolrooms)

Un programme à part du practice tool. Il cherche tout seul comment faire passer **le joueur et la
bouée licorne** à travers le plafond de l'arche des Poolrooms.

En mode automatique, il fait tout seul :
1. lancer le jeu ;
2. charger ta sauvegarde, comme le bouton Continuer ;
3. se placer au départ avec la licorne en main ;
4. enchaîner les essais jusqu'à en réussir un.

Un essai se passe comme ta montée. Le bot recule sur la passerelle, sprinte vers l'arche et, sans
casser sa foulée, enchaîne les boosts : il lâche la bouée dans lui-même, saute dessus dès qu'il la
touche, la reprend, et recommence. Il la lâche une dernière fois en traversant le plafond. Si elle
reste coincée dans le toit et qu'elle en dépasse, il la reprend depuis le toit, comme saopy.
L'essai est **gagné** si le joueur passe le toit avec la bouée. Sinon, le bot se remet en place,
récupère la bouée et recommence.

Il ne tire pas tout au hasard. Il fait varier la longueur de son élan, son décalage, la direction
de sa course (vers l'arche, à ±100°), l'endroit où il regarde (libre : c'est ce qui décide de quel
côté la bouée tombe), s'il sprinte, l'endroit du premier Q, le stick pendant la montée, ses délais,
le nombre de boosts et le moment du dernier lâcher. Il garde ses meilleurs
essais et teste surtout des variantes proches, donc il s'améliore au fil des essais. Il retient ce
qu'il a appris d'une session à l'autre (fichier `bot_bouee.txt`), y compris les essais du premier
bot, qui était immobile au départ.

Il utilise les vraies fonctions du jeu : lâcher (Q), reprendre (E), sauter, le stick de
déplacement et le sprint. Il remet la stamina au maximum à chaque essai.

## Plus rapide

- **Mode rapide** (`rapide = oui`, par défaut). Chaque image du jeu avance de 1/60 s, et le jeu
  enchaîne les images aussi vite que le PC le permet. Une image sur 4 seulement est affichée. La
  physique est donc celle d'un jeu à 60 images/s, mais en accéléré. Toutes les 20 tentatives, la
  console affiche combien d'essais passent par minute et de combien le jeu va plus vite que le
  temps réel.
- **Vérification.** Un essai réussi en mode rapide est rejoué une fois à vitesse normale. C'est
  `GAGNE` seulement s'il passe aussi comme ça.
- **Essais ratés écourtés.** Dès que le joueur retombe sous le plafond, l'essai s'arrête : il ne
  peut plus réussir.
- **Départ sur l'arche.** Si tu sais monter sur l'arche avec la bouée, fais-le, puis tape `stop`,
  `depart` et `auto`. Le bot ne cherchera alors que la traversée du toit.
- **Plusieurs copies du jeu** (`copies = 3`), si le PC suit.

## Enregistrer ta montée

Tape `enregistre`, fais ta montée, puis retape `enregistre` (ou attends 30 s). Le fichier
`demo_<nombre>.txt` qui apparaît à côté du bot contient, image par image, ta position, ton
orientation, ton stick, le sprint, le contact au sol, la bouée en main ou non et sa position, avec
chaque Q et chaque E. Il sert à comparer ta technique avec celle du bot. Tape `stop` d'abord si le
bot tourne.

Marche avec la version 0.8.0 du jeu. En 0.7.35, il n'y a pas de sauvegarde à charger : va dans
les Poolrooms toi-même.

## Première fois : enregistrer le départ

1. Vérifie le chemin du jeu dans `BotBouee/bot_bouee.cfg`.
2. Lance `BotBouee/bot_bouee.exe`. Il démarre le jeu, attend qu'il soit prêt et injecte le bot.
   Une fenêtre console « Bot bouée » s'ouvre. **Ne la ferme pas** : fermer la console d'un jeu
   ferme le jeu (son bouton de fermeture est désactivé).
3. Va dans les Poolrooms, prends la licorne et mets-toi au pied de l'arche, là où la technique
   commence.
4. Tape `depart` dans la console. Le bot cherche aussitôt. Ne touche plus au jeu.

Le départ est gardé dans `bot_bouee.txt`.

## Les fois suivantes

Ta sauvegarde doit être dans les Poolrooms. Lance `bot_bouee.exe` : le jeu démarre, le bot clique
sur Continuer depuis le menu, se place au départ et cherche jusqu'à réussir. Il n'y a rien à faire.

Quand un essai réussit, il s'arrête et affiche `GAGNE !`, avec les réglages qui ont marché. Tape
`vitesse 0.25` puis `rejouer 1` pour le revoir au ralenti.

Chaque essai affiche une ligne, par exemple :

```
#42 toi : en bas (+3.1 m / plafond +4.6) | bouee : tombee de 2.0 m | 4 boosts, bouee hors de portee
#43 REUSSI | toi : sur le toit (+5.2 m / plafond +4.6) | bouee : contre le toit (+6 cm) | 5 boosts, dernier Q au plafond
    elan 2.5 m, decale +0.10 m, angle -8, 1er Q a +0.4 m, avance 0.9 / -0.1, saut 40 ms, E 60 ms, Q 120 ms, dernier Q -0.6 m, 5 tours
```

La fin de ligne dit pourquoi les boosts se sont arrêtés : `dernier Q au plafond`, `bouee hors de
portee` (trop loin pour la reprendre), `boosts finis sous le plafond`, `bouee plus en main` ou
`trop long`.

## Réglages (`bot_bouee.cfg`)

| Réglage | Effet |
|---------|-------|
| `jeu` | Chemin de `Inside the Backrooms.exe`. |
| `copies` | Nombre de copies du jeu à lancer en même temps (de 1 à 8), si le jeu l'accepte. |
| `auto` | `oui` : charge la sauvegarde et cherche tout seul. `non` : il attend tes commandes. |
| `rapide` | `oui` : essais accélérés (voir plus haut). `non` : vitesse normale. |
| `vitesse` | Vitesse du jeu pendant les essais, de 0.25 à 4. Mieux vaut laisser 1 et utiliser `rapide`, qui garde la même physique. |

## Plusieurs bots en même temps

Mets `copies = 3`, par exemple. Le lanceur démarre les copies une par une et injecte le bot dans
chacune. Elles chargent la sauvegarde, prennent le même départ et partagent leurs essais par
`bot_bouee.txt` : chacune part des meilleurs essais de toutes. Le bot force le jeu à continuer de
tourner quand sa fenêtre n'est pas au premier plan.

Si le jeu refuse d'être lancé plusieurs fois, le lanceur le dit et continue avec une seule copie.

## Commandes de la console

| Commande | Effet |
|----------|-------|
| `depart` | Ta position, ton orientation et la bouée en main deviennent le départ (au pied de l'arche, ou sur l'arche). |
| `rejoindre` | Prend le départ enregistré (utile en mode manuel, dans une autre copie). |
| `lancer [n]` | Lance n essais (200 par défaut). |
| `stop` | Arrête, et coupe le mode auto pour cette session. |
| `auto oui` / `auto non` | Remet ou coupe le mode automatique. |
| `rapide oui` / `rapide non` | Essais accélérés ou non. |
| `vitesse x` | Vitesse du jeu pendant les essais. |
| `meilleurs` | Les 10 meilleurs essais. |
| `rejouer i` | Rejoue le i-ème meilleur, à vitesse normale. |
| `enregistre` | Enregistre ta propre montée (retape pour finir). |
| `aide` | La liste des commandes. |

## Limites

- Le bot ne reprend la bouée (E) que si elle est à moins de 2,5 m de la caméra. C'est une estimation
  de la portée du jeu : le vrai E vise un objet.
- La reprise finale depuis le toit ne se fait que si la bouée dépasse du toit (sa boîte de
  collision passe au-dessus de la surface du toit) et qu'elle est à portée.
- L'élan suit la direction opposée à ton orientation au départ. Le bot raccourcit l'élan s'il
  y a un mur ou plus de sol derrière (la piscine).
- Pendant les essais, ne touche pas au jeu et n'active ni le noclip ni la caméra libre du practice
  tool : ils bloquent ou déplacent le joueur à la place du bot.
- Si le jeu change de niveau, le bot se réinstalle tout seul quelques secondes après (en mode auto).

## Compiler

```
cargo build --release
```

Copie ensuite `target/release/float_bot.dll` et `target/release/bot_bouee.exe` dans `BotBouee/`.
