# Journal des décisions

| Date | Décision | Raison |
|---|---|---|
| 2026-11-15 | Monde torique de 100 km × 100 km, voxels de 1 m | Monde fini et sans bord, pas de projection impossible sur une sphère |
| 2026-11-15 | Latitude `φ = asin(d)`, couture = ligne des pôles | Répartition des surfaces par latitude identique à celle d'une sphère |
| 2026-11-15 | Saisons atténuées à zéro près de la couture (δ_eff) | Hauteur du soleil et climat continus à travers la couture |
| 2026-11-15 | Bande crépusculaire : soleil sous l'horizon, aurores, pas d'ombres | La direction du soleil s'inverse forcément à la couture (topologie) ; on cache ce basculement |
| 2026-11-15 | Fuseaux horaires (heure solaire selon la longitude) | Réalisme ; jour et nuit visibles sur la carte |
| 2026-11-15 | 1 jour = 1 h réelle, 365 jours par an, temps lié à l'UTC réelle | Déterminisme ; le site web peut calculer l'heure seul |
| 2026-11-15 | Obliquité fixe de 23,44° | Saisons terrestres |
| 2026-11-15 | Climat stocké en harmoniques annuelles | Transitions continues entre les mois, données compactes |
| 2026-11-15 | Chunks cubiques de 32³ ; grilles macro de 160 m et 32 m | 100 000 = 32 × 3 125, grilles alignées sur les chunks |
| 2026-11-15 | Profondeurs de croûte réduites à environ 1/10, relief jusqu'à environ 3 000 m | Mines praticables, reliefs crédibles sur un petit monde |
| 2026-11-15 | Magme infini et mortel sous le Moho | Limite physique du monde vers le bas |
| 2026-11-15 | Tectonique avec points chauds (archipels de type Hawaï) | Diversité géographique |
| 2026-11-15 | Rust de bout en bout ; Bevy + egui pour le visualiseur | Performance, un seul langage, compilateur strict utile au travail avec l'IA |
| 2026-11-15 | Seul le serveur génère ; on stocke la carte macro et les modifications des joueurs | Taille du monde (10¹⁰ colonnes) ; pas besoin de déterminisme flottant entre machines différentes |
| 2026-11-15 | Code en anglais, documentation en français | Convention de l'auteur |
| 2026-11-15 | Résolution réduite par facteurs 1/5 et 1/25 (et non 1/4) | 625 n'est pas divisible par 4 ; ces facteurs gardent les grilles alignées sur les chunks |
| 2026-11-15 | Bevy minimal (fenêtre + egui) dès M0 | Figer les versions tôt et détecter en CI les dépendances système (Linux) |
| 2026-11-15 | Versions figées : bevy =0.20.1, bevy_egui =0.43.1, serde =1.0.229, jiff =0.2.45, thiserror =2.0.21, anyhow =1.0.104, clap =4.6.7, toml =1.2.1, rand =0.10.4, rand_chacha =0.10.0, image =0.25.10, blake3 =1.8.7, proptest =1.12.0 ; édition 2024, rust-version 1.97 | Dernières versions stables au démarrage ; toute mise à jour est une décision explicite |
| 2026-11-15 | Temps en secondes Unix (POSIX, sans secondes intercalaires) depuis `GAME_EPOCH` ; `t < 0` valide (années négatives) ; `GAME_EPOCH` par défaut = 2026-01-01T00:00:00Z | Calcul simple et identique côté serveur et site web |
| 2026-11-15 | Azimut mesuré depuis le nord (−z) vers l'est (+x), dans [0, 2π) ; direction du soleil = (cos e·sin A, sin e, −cos e·cos A) en axes monde | Convention géographique usuelle, cohérente avec z vers le sud |
| 2026-11-15 | `delta(a, b)` vaut +S/2 dans le cas d'égalité exacte, dans les deux sens (seule exception à l'antisymétrie) | Conforme à la spec (§1.2) ; documenté et testé |
| 2026-11-15 | `bevy/dynamic_linking` derrière la feature `dev` du viewer (alias `cargo viewer-dev`) | Accélère la compilation en développement sans affecter les builds release ni la CI |
| 2026-11-15 | Bande polaire sous couvert nuageux permanent (opaque jusqu'à S_LIGHT, dégagé à S_CLEAR) | On cache la voûte céleste, qui ne peut pas être continue à la couture ; e_vis est conservé pour la lumière diffuse |
| 2026-11-15 | Aurores en anneau au bord de la bande, visibles de loin | Elles signalent l'approche du pôle |
| 2026-11-15 | Le couvert polaire est climatique (nuages et précipitations forcés), pas seulement visuel | Cohérence entre ce qu'on voit et le climat (albédo, neige ou glace permanente) |
| 2026-11-15 | Lune : lunaison de 29,53 jours de jeu, nœuds fixes. Étoiles : catalogue réel, 366 rotations sidérales par an | Ciel nocturne réaliste avec un modèle simple |
| 2026-11-15 | Plaques déplacées par sauts entiers de cellules, depuis une translation cumulée exactement (f64) ; nombre de pas = `total_displacement_m / taille de cellule` (10, 50, 250) ; pas de rotation des plaques pour l'instant | En semi-lagrangien au plus proche voisin, un déplacement sous-cellule est perdu ; cela garde les plaques lentes mobiles à toute résolution. La rotation (« éventuelle » au §4.1) est reportée |
| 2026-11-15 | Chaque pas est un *gather* parallèle depuis l'état précédent ; égalités tranchées par des règles fixes (continent > océan, puis océan le plus jeune, puis id de plaque le plus bas ; brèche attribuée à une plaque bordante, en rotation selon le pas), jamais par l'ordre de rayon | Déterminisme indépendant du nombre de threads (testé avec 1 et 8 threads) |
| 2026-11-15 | Soudage des plaques en collision continentale : après `collision_shortening_m` (5 km) de convergence, les deux groupes prennent une vitesse commune (moyenne pondérée par le nombre de cellules), contacts détectés avant le déplacement | Sans forces, des vitesses constantes détruisaient la croûte continentale sans limite (0,30 → 0,19) ; le raccourcissement reste réel mais borné et la fraction finale concorde entre résolutions |
| 2026-11-15 | Fraction continentale vérifiée en fin de simulation (cible ± `continental_fraction_tolerance` = 0,08), rapportée par la CLI et testée | Précision de l'auteur pour M1 |
| 2026-11-15 | Taille effective minimale des éléments étroits (fosses, arcs, points chauds, orogènes) : `min_feature_cells` = 1,5 cellule | Garder ces éléments visibles en 1/25 (cellules de 4 km) |
| 2026-11-15 | Arcs insulaires = crête immergée (`island_arc_ridge_m` = −800 m) ponctuée d'îles là où un bruit périodique le long de l'arc est fort (sommets `island_arc_peak_m`) | Des crêtes émergées continues formaient des anneaux de terre irréalistes |
| 2026-11-15 | Points chauds actifs sur une fraction (`hotspot_duty`) de chaque `hotspot_island_spacing_m` de déplacement de la plaque, et en continu sous une plaque quasi immobile | Îles régulièrement espacées quelle que soit la vitesse ; un rythme fixe en Ma donnait une crête continue ou des îles trop éloignées |
| 2026-11-15 | `moho_depth` = profondeur du Moho sous le niveau de la mer (m, positive vers le bas) ; `crust_thickness` = épaisseur de la surface au Moho | Lecture directe par la génération de chunks (§4.6) |
| 2026-11-15 | Format `.bin` : en-tête `NSCL`, version 1, type (u8/u16/f32), largeur, hauteur, taille de cellule (f64), unité UTF-8, puis données little-endian ligne par ligne ; `generator_version` = 1 | Format simple, typé et autodescriptif (§6) |
| 2026-11-15 | `rayon =1.12.0` pour les passes cellule par cellule | Parallélisme sans effet sur le résultat |
| 2026-11-15 | Mode Carte dessiné avec egui : texture en filtrage *nearest* et répétition `Repeat`, UV au-delà de [0, 1] | Équivaut à « UV en fract » (§7) sans matériau personnalisé ; les shaders 2D de Bevy 0.20 sont passés en WESL |
| 2026-11-15 | Filtre majoritaire sur les ids de plaque à chaque pas (une cellule n'ayant qu'un voisin de sa plaque rejoint une plaque qui en a trois) | Supprime les cellules isolées qui emmêlaient les frontières en « spaghettis » |
| 2026-11-15 | Les arcs irréguliers qui restent sont acceptés pour M1 ; à revoir avec les courants en M4 | Décision de l'auteur |
| 2026-11-15 | Les rifts en damier sont conservés : leurs petites inclusions océaniques (< `inland_water_max_km2` = 50 km²) deviennent des eaux intérieures à −80 m, sans plateau, comme des lochs écossais | Décision de l'auteur, à condition qu'ils donnent des zones de lacs façon Écosse |
| 2026-11-15 | Fraction continentale calée sur la fraction *finale* : la tectonique est relancée (4 fois au plus) en corrigeant la fraction initiale de l'écart observé ; tolérance ±0,02 | La perte en collision dépend de la graine ; on obtient 0,296 à 0,303 |
| 2026-11-15 | Marge continentale : plateau de 0 à −150 m sur les marges passives, puis talus (`slope_width_m` = 2,5 km) vers l'océan profond ; plateau minimal sur les marges actives. La largeur du plateau est choisie pour qu'il occupe `shelf_share` = 20 % de la croûte continentale (comme sur Terre), dans la limite de `shelf_width_m` = 5 km | Sur un monde de 100 km, un plateau fixe de 5 km noyait 60 à 80 % des continents ; en 1/5 on obtient 1,2 km |
| 2026-11-15 | Altitudes calées par rang sur l'hypsométrie terrestre ramenée aux cibles du §3 (bandes dans `[tectonics.hypsometry]`), séparément pour les terres et l'océan profond ; plateaux, talus, arcs et édifices ajoutés ensuite | L'ordre physique est conservé (dorsales, fosses, orogènes) ; avec 30 % de croûte continentale on obtient environ 25 % de terres et 5 % de plateaux |
| 2026-11-15 | Édifices de points chauds : 6 000 m au-dessus du plancher, perte de hauteur sur 120 Ma | Quelques îles émergent sur chaque graine (testé) |
| 2026-11-15 | `generator_version` = 2 | Marges, hypsométrie, filtre majoritaire et calage modifient le résultat |
| 2026-11-15 | continental_fraction portée à 0,35 (fraction finale) | Le plateau continental noie une partie de la croûte. Objectif : environ 29 % de terres émergées, comme sur Terre |
| 2026-11-15 | Objectif de performance : 60 i/s en 1080p, réglages maximum, sur un portable Linux (Ryzen 5 5600H, RTX 3050 Mobile), mesuré par un benchmark scripté (ms CPU et GPU par image) | Décision de l'auteur ; remplace « 60 i/s sur une RTX 5060 » au §7 |
| 2026-11-15 | Horizon (niveaux de détail en maillage de hauteurs) avancé de M6 à M2 | Décision de l'auteur : le vol libre n'a de sens qu'avec un horizon lointain |
| 2026-11-15 | Altitude des chunks provisoirement tirée de la grille tectonique de 160 m (bicubique périodique + bruit de détail), jusqu'à la grille de 32 m de M3 | Décision de l'auteur ; la grille de 32 m n'existe qu'après l'érosion |
| 2026-11-15 | Génération des chunks jusqu'au Moho (pierre générique, puis magme) à partir de M4 ; la colonne géologique réelle en M5. En M2, seuls quelques chunks sous la surface | Décision de l'auteur |
| 2026-11-15 | CI réduite au job Ubuntu, non bloquante pour l'avancement ; Windows est testé localement par l'auteur | Décision de l'auteur |
