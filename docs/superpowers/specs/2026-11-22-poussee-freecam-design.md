# Poussée d'inspection, freecam, hitbox nommées, réglages séparés

Date : 2026-11-22. Le practice tool est une DLL Rust injectée (hudhook + ImGui) qui résout tout par nom via l'API il2cpp.

## 1. Intention

Le joueur speedrun avec un clip d'inspection. Allongé, dos au mur, il inspecte une bouteille depuis son inventaire. La copie de l'objet apparaît devant ses yeux et sa boîte de collision le pousse en arrière, dans le mur. Il réinspecte une ou deux fois pour traverser.

Ça marche au mur 1 et rate au mur 2. Il veut **voir** ce qui se passe, **comprendre** pourquoi, et **trouver le réglage** (angle du regard, nombre d'inspections) qui marche à un mur donné.

Il veut aussi que tout le tool soit très réglable : une case par chose, rien de caché derrière une autre case, et un nom sur chaque hitbox affichée.

## 2. Objectifs

1. **Poussée, mesure en direct.** À chaque inspection, dessiner la boîte de l'objet et la poussée subie. Afficher recul, montée et côté, à l'ouverture puis au bilan, plus un historique des essais.
2. **Poussée, test en boucle (bot).** Depuis une savestate collée au mur, il essaie des angles pour une chaîne de 1 à 4 inspections. Il trouve la suite qui fait traverser et donne sa fiabilité, comme le bot de la bouée.
3. **Poussée, prévisualisation.** Avant d'inspecter, il montre où la boîte va apparaître et la poussée prévue (vert, orange ou rouge). Il affiche aussi l'écart entre prévu et mesuré.
4. **Freecam.** Une caméra libre pour regarder la scène de l'extérieur (la poussée, mais aussi le reste). Le joueur et la logique du jeu ne sont pas touchés.
5. **Hitbox nommées.** Chaque hitbox dessinée peut afficher ce qu'elle est (type, nom de l'objet). Un mode montre seulement celle qui est visée.
6. **Réglages séparés.** Défaire les trois couplages listés au §7.

**Non-objectifs**
- Le multijoueur : le tool reste pour le solo, en hôte.
- Lire le code natif de la physique : on mesure.
- Toucher aux fichiers du jeu.
- Sauvegarder les nouveaux réglages dans le `.cfg`. Seules les nouvelles touches y vont (§8).

**Anti-objectifs**, ce qui rendrait le travail raté même s'il « marche » :
- Casser une fonction existante, en particulier le bot de la bouée.
- Encombrer l'écran par défaut : les nouveaux affichages sont désactivés au départ.
- Présenter la prévisualisation comme sûre sans montrer sa précision mesurée.
- Faire bouger la vraie caméra du joueur pour la logique du jeu en freecam. Le point d'apparition de l'objet serait faux et fausserait la poussée.

## 3. Ce qui est déjà établi (code natif v0.7.35 + journaux du joueur)

### Inspection (`ItemInspectionUI`, singleton `get_Singleton`)
- **Appels :** `InspectInventoryItem(string itemName)` (depuis l'inventaire), `InspectItem(string itemName)`, `CloseUI()`, `get_IsInspecting()`.
  - `ShowInspectable(InspectableEntry)` fait le placement.
  - `m_CurrentInspectable` (GameObject) est non nul pendant l'inspection, et `CloseUI` le remet à null.
- **Champs (par nom) :** `inspectionAnchor`, `inspectionPivot` (Transform), `inspectionCamera` (Camera), `m_CameraDirection`, `m_CurrentAutoDistance`, `m_CurrentZoomMultiplier`, `m_InspectablePool` (Dictionary<string, InspectableEntry>).
- **Placement :** ancre = pivot = `Player.Camera.transform.position + forward * 0.1`. Le pivot est remis à la rotation identité. La copie est enfant du pivot, en rotation locale identité, couche 30.
  - **La boîte est donc toujours alignée sur les axes du monde**, quelle que soit l'orientation du joueur.
  - `AddInspectionCollider` ajoute un **BoxCollider** aux bornes du mesh de la copie.
- **La caméra d'inspection garde sa position entre deux inspections.** Sa direction est `normalize(ancienne position caméra − ancre)`.
  - Elle sert seulement au sens de la souris : déplacement = `cam.right·dx + cam.up·dy`, borné à 0,15 m.
  - Rotation = autour du haut du monde (dx) et de `cam.right` (dy).
  - Elle n'a **aucun effet sur la poussée à l'ouverture**.
- **Côté joueur :** `PlayerController` utilise un `CharacterController`.
  - `BasePlayerController.IsCrawling` / `IsCrouched`.
  - `PlayerController.SetCrawling(bool)`, `SetCrouchState(CrouchState)`.
  - `lookDirection` sert d'accumulateur du regard (déjà utilisé par le tool, `off_look`).
  - `InputLookDir` / `InputMoveDir` (Vector2).

### Ce que montrent les journaux (allongé, bouteille, autodistance 0,262)

Les mesures viennent du journal externe en lecture seule `speedrun_tools/inspect_logger.py`, dans le dossier du jeu v0.7.35, et de ses fichiers `logs/*.jsonl`. Le code de mesure peut s'en inspirer.

**Convention des angles**, à reprendre telle quelle partout (HUD, bot, historique) :
- **« Regard vers le bas »** = `asin(-forward.y)` du `forward` monde du Transform de `Player.Camera`. Ce n'est pas `lookDirection` du contrôleur.
- **« Orientation carte »** = `atan2(avant.x, avant.z)` du repère joueur (§4).
- **La poussée à l'ouverture** se mesurait comme `yeux_lus − (ancre − 0,1·forward)`. Avec le hook de capture (§6.1), elle se mesure directement.
- **Toute la poussée arrive à l'ouverture**, avant toute action de la souris. Faire tourner la bouteille ensuite ajoute au plus 5 cm.
- **La poussée est droit en arrière ou droit vers le haut**, jamais en biais.
- **Mur 1** (orientation carte ≈ 180°) :
  - 1re inspection à 80° vers le bas : recul 0,30 m, il reste.
  - 2e à 16–25° : recul 0,37 m, il traverse.
  - Environ 0,72 m au total.
- **Mur 2** (orientation ≈ 90°) :

  | Regard vers le bas | Résultat |
  |---|---|
  | 70–80° | **monte** de 0,27 m |
  | **≈ 62°** | **recul 0,27 m qui reste (3/3)** |
  | 10–55° ou vers le haut | recul de 0 à 0,26 m, puis le mur le recrache |

  - 2e inspection à ≈ 0° : encore 0,29 m (0,56 m au total), puis il est bloqué.
- **Seuil apparent :** à partir d'environ 0,27 m de recul il reste dans le mur. En dessous, il revient à son point de départ dans les 1,5 s.
- **Hypothèse à confirmer :** comme la boîte est alignée sur le monde, l'orientation sur la carte change la poussée.

## 4. Contraintes

- **Style du code :** Rust, `hudhook` 0.9.3. Tout est résolu par nom dans `resolve()` et renvoie une erreur lisible. Respecter le style de `game.rs` : `G`, `State`, `Action`, `tick()` après `PlayerController.Update`, et le dessin dans `begin_camera`.
- **Versions du jeu :** 0.8.0, 0.7.35 et 0.7.0. Si une méthode ou un champ manque dans une version, la fonction concernée se désactive proprement (`Option`), comme `boundaries` ou `ignore_pair`. Le reste du tool continue de marcher.
- **Texte des menus en ASCII,** sans accents, comme l'existant (« Arreter », « deplacement »).
- **Tests :**
  - `cargo test` pour la logique pure : classement d'un essai, recherche du bot, repère joueur, calcul de poussée prévue, couplage des options.
  - `selftest.py` doit rester vert : toutes les nouvelles résolutions de noms passent.
- **Le dépôt a environ 1 000 lignes modifiées non commit** (bot de la bouée, README). Elles doivent être commit avant de commencer, et le nouveau travail se construit par-dessus.
- **Repère joueur :** il se calcule depuis le vecteur *droite* de la caméra du joueur, pas depuis son *avant*, qui s'annule quand on regarde le sol. Avant = (−r.z, 0, r.x).

## 5. Approches envisagées

**Pour la poussée :**
- **Mesure seule :** simple, montre le mécanisme, mais ne trouve pas le réglage.
- **Bot seul :** donne la réponse, mais en boîte noire.
- **Prévisualisation seule :** fragile, parce qu'elle dépend d'une physique non lue.
- **Choix du joueur : les trois.** Ordre : mesure → freecam → bot → prévisualisation. La mesure est la base des deux autres, et la prévisualisation est calibrée par les mesures du bot.

**Pour la freecam :**
- **(a) Déplacer la caméra du joueur pendant `Update`.** Rejeté : le point d'apparition de l'objet et les interactions partiraient de la freecam.
- **(b) Une caméra Unity séparée,** activée à la place de celle du joueur.
- **(c) Déplacer la caméra du joueur seulement pour le rendu,** entre `BeginCameraRendering` et la fin du rendu, puis la remettre.

Le choix entre (b) et (c) est laissé au builder (§9). La contrainte : la logique du jeu voit toujours la vraie pose.

## 6. Conception

### 6.1 Poussée, mesure en direct (section de menu « Poussee (inspection) »)
- **Capture exacte à l'ouverture.** Hook de `ShowInspectable`, ou de `InspectInventoryItem` / `InspectItem` si plus simple.
  - **Avant l'appel original :** position et rotation de la caméra du joueur, position du corps, couché ou accroupi, nom de l'objet.
  - **Après l'appel :** la BoxCollider de la copie (`GetComponentInChildren<BoxCollider>` sur `m_CurrentInspectable`), avec centre et taille en monde, et l'ancre.
- **Mesures, dans le repère joueur de l'ouverture :**
  - poussée « à l'ouverture » (premières frames) ;
  - maximum pendant l'inspection ;
  - **bilan** N secondes après la fermeture (réglable, 1,5 s par défaut) ;
  - recul, montée et côté pour chacune.
- **Mur traversé ou pas.** À l'ouverture, un raycast vers l'arrière, à hauteur du corps, trouve la face du mur et sa face arrière, donc son épaisseur. On affiche « enfoncé de X / épaisseur Y », puis « traversé » ou « recraché ».
  - Si aucun mur n'est trouvé à moins de 1,5 m (réglable), « pas de mur derrière ».
- **Dessin, chaque élément ayant sa case :**
  - la boîte de la copie ;
  - la capsule du CharacterController au départ (fantôme) et maintenant ;
  - une flèche du départ à la position actuelle ;
  - la trace du corps jusqu'au bilan ;
  - les faces du mur trouvées.
- **HUD :** une ligne « Poussee : recul / monte / cote » pendant et après l'inspection.
- **Historique :**
  - colonnes : numéro, objet, orientation carte, regard vers le bas, pose, poussée à l'ouverture, bilan, verdict (couleur) ;
  - limite de lignes réglable, bouton « Effacer » ;
  - entrée par entrée : bouton « Sauver la position de depart comme slot ».

### 6.2 Freecam (section « Freecam », touche assignable)
- **Activation :** la vue part de la tête du joueur. ZQSD / Espace / Ctrl / Shift, comme le noclip, avec un curseur de vitesse.
- **Souris :**
  - case « la souris tourne la camera », active par défaut ;
  - option « seulement en gardant clic droit », pour pouvoir inspecter en freecam.
- **Le joueur reste simulé.** Case « bloquer les controles du joueur », active par défaut : les entrées de déplacement et de regard du joueur sont mises à zéro.
- **La logique voit toujours la vraie caméra du joueur.** Inspection, interactions et calculs du tool sur le regard restent ceux de la vraie tête.
- **Affichages :**
  - case « dessiner la capsule du joueur » (le corps est caché en vue première personne) ;
  - case « cacher la vue d'inspection », pour voir le monde pendant une inspection.
- **Boutons :** « Revenir a la tete » et « Teleporter le joueur ici ».
- Les hitbox et dessins du tool restent alignés sur l'image en freecam.

### 6.3 Poussée, test en boucle (bot)
- **Préparation :**
  - une savestate collée au mur, dans la pose voulue, l'objet dans l'inventaire ;
  - les savestates enregistrent et rendent maintenant la pose (debout, accroupi, couché), via `SetCrawling` / `SetCrouchState` ;
  - réglages : slot, objet (liste de l'inventaire), longueur de la chaîne (1 à 4), regard mini / maxi / pas (85° à −60°, pas 2,5° par défaut), durée d'inspection ouverte (0,5 s), attente après fermeture (1,5 s), répétitions des meilleures suites (5).
- **Un essai :**
  1. Recharger le slot, puis quelques frames de stabilisation.
  2. Pour chaque inspection k de la suite : mettre le regard à l'angle choisi, appeler `InspectInventoryItem(objet)`, attendre, appeler `CloseUI`, attendre.
  3. Mesurer comme au §6.1.
- **Recherche :**
  - balayer l'angle de l'inspection 1 ;
  - garder les angles où le joueur reste dans le mur (bilan au-dessus du seuil, ou « enfoncé ») ;
  - pour chacun, balayer l'inspection 2 en rejouant la 1re, et ainsi de suite ;
  - quand un endroit marche, revenir plus souvent dessus, comme `next_cell`.
- **« Réussi » = traversé** (face arrière du mur dépassée et pas recraché au bilan).
- **Résultats :**
  - par inspection, les angles essayés en couleur (traversé, reste dans le mur, recraché, monte) ;
  - la meilleure suite (« 1re 62°, 2e 0°, 3e 60° ») avec son taux de réussite ;
  - « a 2 deg pres » : la fiabilité autour ;
  - « Sauver comme slot » ;
  - le HUD dit de combien lever ou baisser la tête pour l'inspection en cours.
- **Arrêt propre** avec un message clair si : pas d'interface d'inspection, objet absent, joueur mort, slot introuvable, ou bouton « Arreter ».

### 6.4 Poussée, prévisualisation
- **Cache de boîte :** la taille et le centre de la boîte de chaque objet sont mémorisés à la première vraie inspection (ou lus dans `m_InspectablePool`).
- **En permanence, quand c'est coché :**
  - la boîte est placée en monde à `tete + 0,1·avant`, alignée sur les axes ;
  - on calcule la poussée prévue sur la capsule du CharacterController : direction et distance de séparation.
    - **Calcul principal : un calcul boîte/capsule maison**, en fonction pure testée par `cargo test`. `Physics.ComputePenetration` n'existe pas en 0.7.0.
    - `ComputePenetration` peut servir de contrôle là où il existe *(choix du concepteur)* ;
  - **vert** si recul ≥ seuil (0,27 m réglable) et montée faible, **orange** si recul sous le seuil, **rouge** si ça monte.
- **Limite affichée :** seule la boîte est prise en compte, pas les murs.
- **Après chaque vraie inspection,** on affiche « prevu X / mesure Y » et l'écart moyen sur les N derniers essais.

### 6.5 Hitbox nommées
- **Chaque collider dessiné peut avoir une étiquette :**
  - type en clair : « mur invisible », « decor », « zone », « TP », « sortie »… ;
  - nom du GameObject ;
  - option « + forme et couche » (Box, Mesh, Capsule, numéro de couche).
- **Réglages :**
  - une case par type pour les étiquettes ;
  - distance max des étiquettes (curseur, 15 m par défaut) ;
  - mode « seulement celle visée » : l'étiquette du collider sous le viseur, affichée aussi dans le HUD.
- La légende des couleurs reste.

### 6.6 Réglages séparés
1. **« Effets VHS / camera off ».** La case actuelle devient un bouton « Tout couper » (touche F7 gardée), plus une case par effet.
   - **« Tout couper » coupe exactement ce que coupe F7 aujourd'hui,** c'est-à-dire les 10 effets de `off_fx` : vignette, aberration, flou de mouvement, panini, grain, flou de profondeur, vision trouble, basse résolution, VHS de la caméra, distorsion. Plus le VHS du jeu (GameManager), soit 11 cases.
   - **On n'ajoute ni balance des blancs ni `colorAdjustments`** *(choix du concepteur)* : le curseur de luminosité écrit dans `colorAdjustments`.
2. **Les hitbox ne coupent plus d'effets en douce.** Case visible « Couper les effets qui deforment l'image pendant les hitbox » (panini, VHS, distorsion), active par défaut pour garder le comportement actuel.
3. **Affichage des colliders, quatre cases indépendantes :** murs invisibles, décor et objets, zones, spéciaux (TP, sortie, dégâts, niveau). Plus de case qui dépend d'une autre.
   - **Les touches gardent exactement leur effet actuel** *(choix du concepteur, pour ne rien changer aux habitudes)* : F8 = murs invisibles, F9 = zones **et** spéciaux ensemble.

## 6.7 Déjà présent après la fusion de `float-aids-and-bot` (2026-11-22)

La branche fusionnée apporte une partie de ce plan. On la complète, on ne la refait pas.

- **Caméra libre (F11, `camera_libre`).** Elle est déplacée seulement pour le rendu (`begin_context` / `end_context`), et la souris passe par `InputLookDir`. C'est l'approche (c) du §5. Le §6.2 se limite à ajouter les options qui manquent : souris seulement au clic droit, case « bloquer les controles », cacher la vue d'inspection, « Teleporter le joueur ici », capsule du joueur au choix.
- **Glitch d'inspection** (section « Glitch d'inspection »), avec `Push` et `track_inspection` :
  - aperçu cyan de la forme de l'objet là où il apparaîtra (après une première inspection) ;
  - capsule blanche ;
  - poussée mesurée (avant, droite, haut) et « A TRAVERSE » si un obstacle solide a été franchi ;
  - bouton pour se donner la ProtectionSuit.

  Le §6.1 enrichit cet existant : poussée à l'ouverture et au bilan, épaisseur du mur, historique, convention des angles. Le §6.4 y ajoute le calcul de poussée prévue et sa couleur.

## 7. Ordre de livraison

1. Commit du travail en cours (par le joueur).
2. Mesure en direct.
3. Freecam.
4. Bot.
5. Prévisualisation.
6. Hitbox nommées.
7. Réglages séparés.

Chaque étape est livrée compilée, `cargo test` et `selftest.py` verts, le README mis à jour.

## 8. Touches

Nouvelles actions assignables dans le `.cfg`, vides par défaut : freecam on/off, mesure de poussée on/off, lancer / arrêter le bot de poussée, prévisualisation on/off.

## 9. Laissé au builder

- **Freecam :** caméra séparée ou rendu seulement (§5), tant que la logique voit la vraie pose.
- **Hook de capture :** `ShowInspectable` ou `InspectInventoryItem` / `InspectItem`, et `ComputePenetration` contre un calcul maison.
- **Organisation :** nouveaux fichiers (par exemple `push.rs`, `freecam.rs`) ou ajouts à `game.rs`, et la disposition exacte des menus.
- **Valeurs par défaut** non fixées ici, et seuils de classement autres que ceux du §6.

## 10. Risques

- **Saisie de la souris en freecam :** l'Input System du jeu lit la souris. Si zéroïser `InputLookDir` ne suffit pas, il faudra une autre méthode (désactiver la map d'actions, ou lire la souris soi-même).
- **Déterminisme du bot :** si rejouer une suite ne donne pas toujours la même poussée, les répétitions et le taux de réussite deviennent essentiels.
- **Version 0.7.0 :** les noms d'inspection et du joueur apparaissent dans les fichiers des trois versions. Seul `selftest.py` sur chaque version prouve qu'ils se résolvent. Si une résolution échoue, la fonction concernée se désactive avec un message.
