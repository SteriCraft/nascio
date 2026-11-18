# Spécification technique du monde — Nascio

Ce document fait autorité. Toute modification se fait avec l'accord de l'auteur et est consignée dans `docs/decisions.md`.
Les valeurs numériques marquées *(config)* sont des valeurs par défaut, modifiables dans la configuration TOML du monde.

---

## 1. Repère et topologie

### 1.1 Axes
- **x** : vers l'est.
- **y** : vers le haut (vertical, non périodique). Le niveau de la mer est à `y = 0`.
- **z** : vers le **sud**. Le nord correspond donc à −z. Ce choix suit la convention de Bevy (y vers le haut, repère direct).
- `WORLD_SIZE = 100_000` blocs (1 bloc = 1 m). x et z appartiennent à `[0, 100 000)` et sont **périodiques**.

### 1.2 Mathématiques du tore (`nascio-core::torus`)
- `wrap(v) = v.rem_euclid(WORLD_SIZE)`.
- `delta(a, b)` : la différence signée la plus courte de a vers b. On calcule `d = (b − a).rem_euclid(S)`, puis on retranche S si `d > S/2`.
- `distance`, voisinage et interpolation passent tous par ces fonctions.
- Les versions entières (`i32`) et flottantes (`f64`) sont toutes deux disponibles.

### 1.3 Chunks
- Chunks **cubiques de 32³**. `ChunkPos { cx, cy, cz }` avec `cx, cz ∈ [0, 3125)` (périodiques) et `cy: i32` sans limite.
- 100 000 = 32 × 3 125 : le monde boucle sur un nombre entier de chunks.
- Positions de bloc : `i32`. Position locale dans le chunk : `u8` (0..32).

### 1.4 Grilles macro (alignées sur les chunks)
| Grille | Taille de cellule | Résolution | Usage |
|---|---|---|---|
| Tectonique | 160 m (5 chunks) | 625 × 625 | plaques, croûte, relief initial |
| Hydrologie / relief fin | 32 m (1 chunk) | 3 125 × 3 125 | érosion, rivières, lacs, altitude finale |
| Climat | 160 m | 625 × 625 | température, précipitations, vents, courants |

Toutes les grilles sont périodiques sur les deux axes. L'échantillonnage à une position quelconque se fait par interpolation **bicubique périodique**, ou bilinéaire quand c'est suffisant.
Pour itérer vite, le CLI doit pouvoir générer à **résolution réduite** avec les mêmes paramètres physiques. Les facteurs de réduction doivent diviser 625 pour que les grilles restent alignées sur les chunks : **1/5** (125² pour la tectonique et le climat, 625² pour l'hydrologie) et **1/25** (25², 125²) pour les tests rapides.

---

## 2. Latitude, longitude, temps

### 2.1 Latitude (répartition équivalente à une sphère)
- `d(z) = (50 000 − z) / 50 000`, compris dans [−1, 1]. `z = 0` (≡ 100 000) est la **ligne des pôles** (« la couture »), et `z = 50 000` est l'équateur.
- **`φ = asin(d)`**. φ > 0 dans l'hémisphère nord (z < 50 000) et φ < 0 dans l'hémisphère sud.
- La **distance à la couture** vaut `s = min(z, 100 000 − z)`, d'où `|d| = 1 − s/50 000`.
- Repères : cercle polaire (66,56°) à s ≈ 4 125 m ; tropiques (23,44°) à environ 19 890 m de l'équateur. Les tropiques couvrent environ 40 % du monde, comme sur Terre.
- Traverser la couture fait passer φ de +90° à −90°. Toutes les grandeurs physiques doivent rester **continues** à cet endroit, comme expliqué ci-dessous.

### 2.2 Longitude
- `λ = 360° · x / 100 000`. Une heure de décalage solaire correspond à environ 4 167 m.

### 2.3 Temps du jeu (déterministe, lié à l'horloge réelle)
- `GAME_EPOCH` : un instant UTC fixé dans la configuration du monde. `t` est le nombre de secondes réelles écoulées depuis `GAME_EPOCH`.
- **1 jour = 3 600 s réelles. 1 an = 365 jours** (environ 15,2 jours réels).
- `day_index = floor(t / 3600)`, `day_frac = (t mod 3600) / 3600`.
- `year = floor(day_index / 365)`, `N = (day_index mod 365) + day_frac`. N est le jour de l'année, **continu**, dans [0, 365).
- « L'heure du jeu » de référence est l'heure solaire de la longitude 0 (x = 0).
- Heure solaire locale : `local_frac = fract(day_frac + λ/360)`. Angle horaire : `H = 2π · local_frac − π` (midi local quand H = 0).

### 2.4 Astronomie
- Obliquité **ε = 23,44°**, fixe pour tous les mondes.
- Déclinaison : `δ(N) = −ε · cos(2π (N + 10) / 365)`.
- **Déclinaison effective**, qui annule les saisons près de la couture :
  `δ_eff = δ · w(s)`, avec `w(s) = smoothstep(0, S_FADE, s)` et `S_FADE = 3 000 m` *(config)*, soit φ ≈ 70°.
  Sur la couture, c'est un équinoxe permanent, ce qui rend la hauteur du soleil continue d'un hémisphère à l'autre.
- Hauteur géométrique du soleil : `sin e = sin φ · sin δ_eff + cos φ · cos δ_eff · cos H`.
- Azimut : formule standard. Le soleil se lève à l'est (+x) dans les deux hémisphères.

### 2.5 Bande crépusculaire de la couture (rendu uniquement)
La **direction** du soleil ne peut pas être continue à travers la couture. Au nord, le soleil de midi est au sud. Après la traversée, il est au nord, puisqu'on continue de marcher vers le nord. On cache donc le soleil sous l'horizon autour de la couture.
- Hauteur visible : `e_vis = e − r(s) · (E_max(s) + 4°)`, avec :
  - `r(s) = 1 − smoothstep(S_DARK, S_LIGHT, s)`, `S_DARK = 1 000 m`, `S_LIGHT = 2 000 m` *(config)* ;
  - `E_max(s) = 90° − |φ(s)| + ε · w(s)`, qui majore la hauteur de midi.
- Garanties, à vérifier par des tests :
  - pour `s ≤ S_DARK`, `e_vis ≤ −4°` à tout instant ;
  - pour `s ≥ S_LIGHT`, `e_vis = e` ;
  - `e_vis` est continue en s et en t.
- Éclairage :
  - l'intensité de la lumière directionnelle est une fonction continue de `e_vis`, nulle pour `e_vis ≤ 0` (fondu entre 0 et +10°). **Elle est donc nulle dans la bande : aucune ombre** ;
  - la couleur du ciel et la lumière ambiante sont des fonctions continues de `e_vis` (crépuscule civil, nautique, nuit) ; la lumière du ciel monte de −6° à +10° ;
  - la hauteur du soleil est calculée **par pixel** (position du fragment), et non une fois pour toute la scène à la caméra ;
  - les aurores ne sont plus calculées ici : voir §2.7.
- Le **climat** utilise `e` et `δ_eff` (insolation physique), pas `e_vis`. Près de la couture, l'insolation tend de toute façon vers 0.

### 2.6 Insolation journalière moyenne (climat)
`Q = (S0/π) · (h0 · sin φ · sin δ_eff + cos φ · cos δ_eff · sin h0)`, avec `h0 = arccos(clamp(−tan φ · tan δ_eff, −1, 1))` et `S0 = 1361 W/m²`.

### 2.7 Bande nuageuse polaire et aurores
La couture est cachée sous un **couvert nuageux permanent**. Ni le soleil, ni la Lune, ni les étoiles n'y sont visibles, ce qui évite la discontinuité de la voûte céleste.
- **Couverture minimale** : `c_min(s) = 1 − smoothstep(S_LIGHT, S_CLEAR, s)`, avec `S_CLEAR = 3 000 m` *(config)*. Le couvert est opaque (`c = 1`) pour `s ≤ S_LIGHT`, c'est-à-dire partout où `e_vis ≠ e`.
- **Couverture rendue** : `c = max(c_min, c_météo)`. La météo locale peut ajouter des nuages, mais jamais en retirer dans la bande.
- `e_vis` (§2.5) est conservé : la lumière diffuse sous les nuages reste une fonction continue de `e_vis` (jour gris, nuit noire).
- **Aurores** : rideaux lumineux situés géographiquement sur un anneau au bord de la bande, à haute altitude, et visibles de loin à l'horizon. Les repérer indique qu'on approche du pôle.
  - Intensité en un point de l'anneau : `a(s) = smoothstep(S_CLEAR, S_AUR, s) · (1 − smoothstep(S_AUR, S_AUR_OUT, s))`, avec `S_AUR = 3 500 m` et `S_AUR_OUT = 5 000 m` *(config)*.
  - Elles ne sont visibles que de nuit : `× (1 − smoothstep(−12°, −6°, e_vis))`.
  - Sous le couvert, elles sont masquées comme le reste du ciel.

### 2.8 Étoiles et Lune (rendu uniquement)
- **Étoiles** : catalogue réel, en coordonnées équatoriales. Orientation locale par φ et par l'angle horaire sidéral `H★ = H + λ_☉`, avec `λ_☉ = 2π(N + 10)/365 − π/2` la longitude écliptique du soleil (déjà implicite dans δ, puisque `δ = ε · sin λ_☉`). Cela donne 366 rotations sidérales par an de 365 jours, et l'équinoxe de mars (N ≈ 81,25) correspond à λ_☉ = 0, comme sur Terre. On utilise λ_☉ à la place de l'ascension droite du soleil : on néglige ainsi l'équation du temps, soit au plus environ 2,5° d'écart.
- **Lune** :
  - lunaison de **29,53 jours de jeu** ;
  - longitude écliptique `λ_☾ = λ_☉ + 360° · (N_total / 29,53) + λ0`, où `N_total = day_index + day_frac` et `λ0` est réglé pour que l'époque tombe à une phase choisie *(config)* ;
  - inclinaison de 5,14° sur l'écliptique, avec des nœuds fixes (pas de précession) ;
  - phase et éclairement déduits de l'angle entre la Lune et le soleil.
- Les étoiles et la Lune sont pondérées par `(1 − c)` : elles sont invisibles dans la bande.
- Ni la Lune ni les étoiles n'éclairent l'intérieur de la bande.

---

## 3. Échelle verticale (cibles)
Le monde est environ 400 fois plus petit que la Terre à l'horizontale. Les profondeurs de croûte sont donc réduites d'environ 1/10, tandis que le relief garde des valeurs crédibles.

| Grandeur | Cible |
|---|---|
| Niveau de la mer | y = 0 |
| Plus hauts sommets | ~ +3 000 m |
| Dorsales océaniques | ~ −900 m |
| Plaines abyssales | ~ −1 800 m (de −1 500 à −2 000 m) |
| Fosses de subduction | jusqu'à ~ −2 500 m |
| Talus continental | de −150 m à l'abysse sur ~5 km, pente ≤ 0,35 |
| Moho sous l'océan | ~ 700 m sous le plancher |
| Moho sous un continent | ~ 3 500 m sous la surface |
| Moho sous une chaîne de montagnes | jusqu'à ~ 6 000 m sous la surface |
| Au-delà du Moho | **magme du manteau**, infini et mortel |
| Pression atmosphérique | profil terrestre (atmosphère standard) comprimé ×2 : 1 013 hPa au niveau de la mer, ~795 hPa à 1 km, ~472 hPa à 3 km, ~0,8 hPa à 25 km, **nulle à 50 km** (ligne de Kármán) *(config `[atmosphere]`)* |
| Gradient thermique vertical | −6,5 °C/km, **non comprimé** ; plancher de la tropopause à −56,5 °C *(config)* |

**Écart assumé à l'hypsométrie terrestre** (revue de M4) : l'océan est moins profond qu'une mise à l'échelle de la Terre (abysse à −1 800 m au lieu de −3 000 m), et les bassins, larges de 20 à 40 km seulement, sont dominés par les talus : environ 17 % du monde sous −1 500 m, 39 % entre −1 500 et −200 m, 15 % entre −200 et 0 m, contre une majorité d'abysse sur Terre. Seules les terres restent calées sur l'hypsométrie terrestre.

L'isostasie (modèle d'Airy) guide la relation entre l'épaisseur de la croûte et l'altitude, mais avec des paramètres **ajustés** pour atteindre ces cibles. On ne cherche pas l'exactitude physique.

---

## 4. Pipeline de génération macro (`nascio-worldgen`)
Ordre : **tectonique → érosion → climat → hydrologie finale → biomes → géologie**.
Chaque étape lit les couches précédentes et écrit les siennes. Chaque couche peut être exportée en PNG.

### 4.1 Tectonique (grille de 625², pas de temps fixes)
Référence : Cortial et al., *Procedural Tectonic Planets* (2019), adaptée à une grille torique.
- **Plaques** : de 8 à 12 *(config)*. On les initialise par un Voronoï périodique déformé par du bruit. Chaque plaque reçoit une vitesse de translation constante. Elle se déplace par cellules entières, à partir d'une translation cumulée exactement. La rotation est reportée.
- **Croûte continentale** : ≈ 35 % de croûte continentale *(config)*, regroupée en quelques masses ; le reste est océanique. Cette fraction est la fraction *finale* : la fraction initiale est calée en relançant la simulation, car les collisions consomment de la croûte. Le plateau continental en noyant une partie, on vise ainsi environ 29 % de terres émergées.
- **État par cellule** : id de plaque, type de croûte (océanique ou continentale), épaisseur, âge de la croûte océanique, altitude, drapeaux d'histoire (orogenèse, arc volcanique, rift, point chaud, ophiolite).
- **À chaque pas** : advection semi-lagrangienne des plaques (périodique), puis traitement des frontières :
  - **divergence** : création de croûte océanique d'âge 0 (dorsale). Sur un continent, création d'un rift ;
  - **subduction océan → continent** : la croûte océanique est consommée, une fosse se forme, et un **arc volcanique** apparaît sur la plaque chevauchante ;
  - **subduction océan → océan** : arc insulaire ;
  - **collision continent ↔ continent** : épaississement de la croûte et soulèvement (orogenèse), avec suture possible d'ophiolites. La collision freine les plaques : après `collision_shortening_m` *(config, 5 km)* de convergence, les deux plaques sont soudées et prennent une vitesse commune (moyenne pondérée par leur nombre de cellules), ce qui borne le raccourcissement ;
  - **frontière transformante** : simple cisaillement.
- **Subsidence océanique** : la profondeur croît comme `√âge`.
- **Points chauds** : de 2 à 5 *(config)*, fixes dans le repère du manteau. La plaque qui défile au-dessus reçoit une suite d'édifices volcaniques dont l'âge augmente avec la distance, ce qui donne des **archipels de type Hawaï**. Ils peuvent aussi tomber sous un continent (trapps, plateaux).
- Le nombre de pas suffit à faire émerger des continents de formes variées. On n'a pas besoin de cycles de Wilson complets.
- **Marges et hypsométrie** : plateau de 0 à −150 m sur les marges passives (plateau minimal sur les marges actives), puis, sur **toutes** les marges, un talus vers l'océan profond sur `slope_width_m` *(config, 5 km)* ; sur une marge active, la fosse commence au pied du talus. Les petites inclusions océaniques (rifts) deviennent des eaux intérieures. Les altitudes des terres sont calées par rang sur l'hypsométrie terrestre ramenée aux cibles du §3 *(config)* ; le fond océanique suit directement la loi profondeur–âge (l'étalonnage par rang, `calibrate_deep`, est désactivé : il transformait les collines abyssales en centaines de mètres de relief).
- **Pente maximale du fond marin** : le fond est relevé là où il descend plus vite que `max_seafloor_slope` *(config, 0,35)* depuis la côte (comptée à 0 m) ou une cellule marine moins profonde. Les chaînes qui plongent dans la mer continuent sous l'eau à cette pente au plus. Le fond ne fait que monter et reste sous le niveau de la mer. À terre, symétriquement, une terre ne dépasse pas `max_coastal_slope` *(config, 0,8)* fois sa distance à la mer : les chaînes ne tombent plus dans la mer en parois, et les sommets éloignés de la côte gardent leur altitude.
- **Sorties** : `plate_id`, `crust_type`, `crust_thickness` (épaisseur de la surface au Moho, m), `crust_age`, `elevation_tectonic` (altitude après marges et calage hypsométrique, m), `moho_depth` (profondeur du Moho sous le niveau de la mer, m, positive vers le bas), `history_flags`, `hotspot_track`.

### 4.2 Érosion (grille de 3 125²)
- Suréchantillonnage de l'altitude tectonique vers la grille de 32 m, avec du bruit périodique de détail.
- Érosion fluviale de type **stream power** (`E = K · A^m · S^n`), plus de la diffusion sur les pentes. Les précipitations sont d'abord estimées grossièrement (zonales et orographiques), car le climat n'est pas encore connu.
- Écoulement **D8 ou MFD périodique**. Les dépressions sont traitées par *priority-flood* (comblement ou lacs).
- **Soulèvement tectonique** pendant l'érosion : un taux de soulèvement tiré des vitesses de convergence actuelles des plaques (§4.1) est appliqué à chaque pas sur les terres proches des frontières convergentes (orogènes de collision, arcs volcaniques). Les plaques soudées, de vitesse commune, ne soulèvent plus.
- **Sédiments côtiers** : le volume érodé en amont de chaque embouchure construit un cône de dépôt sous la mer (pente et profondeur maximale *(config)*), au lieu d'une diffusion marine. Les cônes de dépôt ne montent jamais au-dessus d'un plateau qui s'approfondit vers le large : `−1 m − 2 % × distance à la côte` *(config)*. Les cônes voisins forment ainsi une plateforme en pente, de −1 m au rivage jusqu'à −20 m environ à 1 km, et non un plat à −1 m. La masse est conservée et le cône ne crée jamais de terre. Après l'érosion, la pente maximale du fond marin (§4.1) est appliquée à nouveau sur la grille fine : le front des cônes devient un tablier au lieu d'un mur (ce remblai ajoute du volume).
- **Lacs** : une dépression d'au moins 2 m de profondeur et 20 000 m² devient un lac *(config)* ; les autres sont comblées. Densité de référence : 70 à 115 lacs par monde.
- Durée de 1 Ma *(config)* ; génération complète en pleine résolution en 50 à 60 s environ.

### 4.3 Climat (grille de 625²)
On simule une année type sur 24 à 48 pas, puis on **projette chaque grandeur sur des harmoniques annuelles** :
`X(N) = X0 + A1·cos(2π(N − p1)/365) + A2·cos(4π(N − p2)/365)`.
On stocke `(X0, A1, p1, A2, p2)` par cellule. L'évaluation à n'importe quel instant est **continue par construction**, ce qui garantit des transitions fluides entre les mois.
- **Température** : bilan radiatif simplifié à partir de Q, puis :
  - inertie thermique : déphasage d'environ 1 mois sur terre et 1,5 à 2 mois en mer, amplitude réduite en mer, continentalité selon la distance à la côte : la température de la mer (saison amortie, `sea_amplitude` *(config)*) se mélange à celle de la terre avec un poids `exp(−d / 20 km)` *(config)* ;
  - gradient vertical de −6,5 °C/km ;
  - anomalies dues aux courants océaniques ;
  - la surface de la mer ne descend pas sous le point de congélation de l'eau de mer (−1,8 °C *(config)*), sauf sur la **banquise pérenne** : quand le mois le plus chaud de la mer passe sous ce point, la surface est de la glace et suit l'air, avec un fondu continu sur 5 °C *(config)* ;
  - calibration pour obtenir des moyennes zonales plausibles (≈ 26 °C à l'équateur, ≈ −20 °C en moyenne près de la couture), avec des moyennes tempérées douces : environ 15 °C en juillet sur les côtes vers 45–60° et pas de mer gelée en dessous d'environ 60°.
- **Vents dominants** : trois cellules par hémisphère, définies **en latitude φ** (alizés pour 0–30°, vents d'ouest pour 30–60°, vents polaires d'est pour 60–90°). Les bandes se déplacent avec δ_eff selon la saison. La couture est un anticyclone polaire : les deux côtés divergent, ce qui est cohérent.
- **Courants océaniques** : gyres simplifiés entraînés par les vents et déviés par les côtes. On advecte des anomalies chaudes et froides : courants chauds sur les bords ouest des bassins, remontées froides sur les bords est en zone subtropicale.
- **Humidité et précipitations** : l'air transporte un rapport de mélange de vapeur d'eau (g/kg), advecté par le vent. Sur l'océan, il se recharge vers une humidité relative de 90 % *(config)* à la température et à la pression locales. Sur terre, il s'assèche lentement. **Au-delà de la saturation** `w_s(T, P)` (formule de Magnus, pression du profil ci-dessus), l'excès condense et tombe en pluie. Ainsi, l'air qui monte un relief se refroidit et se détend : pluies orographiques au vent, sécheresse à l'abri. L'air poussé sur une terre plus froide se comporte de même (neige côtière d'hiver). En mer, un air refroidi par le dessous (courants froids) donne du brouillard, sans pluie. L'humidité est advectée relativement à la référence marine de sa latitude : un simple déplacement méridien ne fait pas pleuvoir, car le profil zonal (ZCIT qui suit δ_eff, ceintures sèches vers 30°) porte déjà ces pluies, modulées par l'humidité de l'air.
- **Bande polaire** : couverture nuageuse forcée (`c ≥ c_min(s)`, §2.7) et précipitations renforcées dans la bande, avec un impact réel sur le climat (albédo, neige ou glace permanente). Le facteur de précipitations est en *(config)*.
- **Cycle diurne** (à l'exécution) : son amplitude dépend de l'humidité et de la continentalité. Le maximum est vers 14–15 h solaires locales. Il s'estompe sur les 1 000 premiers mètres au-dessus du sol *(config)*.
- **Air en un point** (à l'exécution, `nascio-core::atmosphere`) : la température vient de la cellule, ramenée à l'altitude par le gradient vertical, plus le cycle diurne. La pression suit le profil comprimé. L'humidité relative découle du rapport de mélange de surface à cette température et à cette pression.
- **Sorties** : harmoniques de `temperature`, `precipitation`, `humidity` (rapport de mélange de l'air de surface, g/kg), `wind` (vecteur), `snow_fraction` (**part du sol couverte de neige**, et non part neigeuse des précipitations), `cloud_cover` (couverture nuageuse, jamais sous `c_min(s)`) ; champ `ocean_current` ; amplitude diurne `diurnal_range`. Chaque grandeur harmonique est stockée en cinq couches (`<nom>_x0`, `_a1`, `_p1`, `_a2`, `_p2`, phases en jours).

### 4.4 Hydrologie finale (grille de 3 125²)
On recalcule les débits avec les précipitations du climat, puis on détermine les rivières (seuil d'accumulation, largeur et profondeur selon le débit), les lacs et les deltas. Une légère passe d'érosion de finition est possible.

### 4.5 Biomes
Classification à partir des harmoniques de température et de précipitations, de type **Köppen** simplifié, ou Whittaker en premier jet. On ajoute des cas particuliers : glaciers et calottes (température moyenne annuelle et altitude), mangroves, marais.

### 4.6 Géologie et ressources
**Contextes géologiques** (étape `geology`, grille de 160 m) : chaque cellule reçoit un poids entre 0 et 1 pour chacun des contextes connus du code. Ces poids viennent du type et de l'âge de la croûte, de l'historique tectonique (`history_flags`), de l'épaisseur des sédiments, du climat et de l'hydrologie :

| Contexte | Définition |
|---|---|
| `ocean`, `ridge`, `trench` | croûte océanique, dorsale, fosse |
| `arc`, `orogen`, `rift`, `hotspot`, `ophiolite` | marques de l'historique tectonique |
| `ultramafic` | ophiolites, et dans une moindre mesure les basaltes des points chauds |
| `craton`, `platform` | croûte continentale ancienne ou plus jeune, hors orogène et hors arc |
| `basin` | couverture sédimentaire épaisse : bassin continental, plateau marin ou marge passive |
| `marine_shelf` | plateau continental (croûte continentale) sous une mer peu profonde |
| `passive_margin` | croûte océanique à moins de 6 km d'un continent et à plus de 5 km d'une fosse ou d'un arc *(config)* |
| `humid`, `arid` | climat actuel, faute de paléoclimat |
| `laterite` | climat tropical humide sur une pente faible |
| `tropical` | climat tropical humide, quelle que soit la pente |
| `river_source` | lit de rivière à l'aval d'un orogène ou d'un arc |
| `coast` | plages et rivages |

Sorties : `geo_context` (contexte dominant), `sediment_thickness` (m), `weathering_depth` (m) et `laterite` (0 à 1).

**Colonne verticale** (de haut en bas) :
1. **Sol** de 0 à 3 m, selon le biome et la pente. On trouve du **sable de quartz** sur les plages, les déserts chauds et les fonds marins peu profonds, et du **gravier** sur le lit des rivières. Chaque biome a un bloc de surface posé sur un bloc de sol, sur une épaisseur donnée *(config `[[voxel.soils]]`)* : herbe sur terre (forêt tempérée, méditerranéen, steppe), herbe sur sol rouge (savane, forêt tropicale), mousse (toundra), podzol (forêt boréale), vase (mangrove, marais, fonds de lac), gravier (désert froid), sable (désert chaud, bandes côtières). L'épaisseur diminue avec la pente jusqu'à la roche nue. L'herbe est un seul bloc partout : sa couleur suit le climat (précipitations et température moyenne annuelles, en dégradé continu), dans les chunks comme dans l'horizon.
2. **Altérites** : régolithe, ou latérite en climat tropical humide. Épaisseur `weathering_depth` *(config)*, nulle sur les fortes pentes ; dans les chunks, elle s'amincit avec la pente du bloc (`weathering_slope`, 0,4 à 1) et avec l'altitude (`weathering_alt_m`, de 2 200 à 3 200 m, jusqu'au quart), puis l'arrondi au bloc est tramé par un hachage de la colonne (frange irrégulière au lieu d'un isocontour de la couche) ; sous climat tropical, le régolithe (saprolite) tient sur des pentes plus fortes.
3. **Couverture sédimentaire**, d'épaisseur `sediment_thickness`, **jusqu'à environ 500 m** *(config)* :
   - épaisse dans les bassins (plaines continentales basses), les rifts, les deltas et cônes de dépôt (dépôts de l'érosion), et sur les plateaux ;
   - sur les marges passives, un coin sédimentaire qui s'amincit vers le large : 45 % de l'épaisseur maximale au pied du continent, 40 % de cela au bord de la marge *(config `margin_fill`)* ;
   - mince sur les cratons et quasi nulle sur les orogènes ;
   - sur l'océan, les sédiments pélagiques s'épaississent avec l'âge de la croûte.

   Les bancs alternent **grès, argile, calcaire, craie et évaporites (gypse)** selon le contexte : calcaire, craie et argile en milieu marin, grès et argile sur les continents (avec du calcaire, et de la craie sur les plates-formes), gypse en climat aride. Leur épaisseur va de quelques mètres à environ 30 m. Ils suivent une altitude absolue légèrement ondulée, comme des strates horizontales.
4. **Socle** :
   - continental : granite et gneiss, ou schistes et roches métamorphiques dans les orogènes ;
   - océanique : basalte en surface, puis gabbro ;
   - arcs et points chauds : andésite et basalte ;
   - ophiolites : basalte, gabbro et péridotite ;
   - rifts : coulées de basalte sur le socle.
5. **Croûte inférieure** (dernier tiers environ avant le Moho) : granulite ou gabbro ;
6. **Moho**, à profondeur variable (§3, tectonique), puis **magme**, infini et mortel. Les chunks lissent la profondeur du Moho de la grille tectonique (trois flous de 2 cellules, environ 1 km) : le passage d'une cellule océanique à une cellule continentale ne fait ni marche verticale de 3 km ni aiguille de magme sous une enclave océanique isolée.

Les limites entre contextes sont déformées par un bruit périodique : elles ne suivent pas les cellules de 160 m.

**Température du sous-sol** (`nascio-core::geotherm`, config `[geotherm]`). La courbe est douce près de la surface et raide près du Moho :

`T(d) = T_s + (T_moho − T_s) · (e^(k·u) − 1) / (e^k − 1)`, avec `u = d / D`.

- `d` est la profondeur sous la surface rocheuse et `D` l'épaisseur de la croûte (de la surface rocheuse au Moho).
- `T_moho = 1 200 °C` et `k = 2,7` *(config)*.
- Sous un continent (`D` = 3 500 m, `T_s` ≈ 10 °C), cela donne environ 50 °C à 500 m, 250 °C à mi-croûte et un gradient d'environ 1 °C/m juste au-dessus du Moho. On peut donc miner sans danger les premières centaines de mètres ; la chaleur ne devient mortelle qu'à l'approche du Moho.
- `T_s` est la température moyenne annuelle de l'air sur les terres. Au fond des mers, c'est la température de l'eau de fond : 2 °C sous plus de 200 m d'eau *(config)*, en raccord avec la surface au-dessus.
- Pour une même profondeur, le gradient est plus fort sous les océans (croûte mince) et plus faible sous les orogènes (croûte épaisse).

**Gisements** : le **catalogue** est un fichier, `config/deposits.toml`. Les types n'y sont pas codés en dur : le code ne connaît que les **contextes** ci-dessus et les **formes**.

Chaque type de gisement décrit :
- le bloc et ses proportions (un mélange est possible, ainsi qu'un « toit » d'un autre bloc pour les couches) ;
- les contextes et leurs poids, et les contextes requis (multiplicatifs ; `"a|b"` prend le plus grand des deux) ;
- facultativement, les roches hôtes (`host_rock`, noms de blocs) : le corps ne remplace que ces roches, par exemple le silex dans le calcaire et la craie ;
- facultativement, un type parent (`above`, un type placé plus haut dans le catalogue, avec des blocs) et une probabilité (`probability`, dans ]0, 1]) : au lieu d'être tirés par tuile (pas d'`abundance`), les corps sont posés sur ceux du parent, un au plus par corps parent, avec la probabilité `probability` × le poids des contextes du type à sa position, centrés sur lui (même `x`, `z`) ; chaque corps parent a son propre RNG (`deposits/<nom>/above/<rang>`). Exemple : le chapeau de cuivre oxydé au-dessus d'un porphyre. Un type placé au-dessus d'un autre n'est ni un placer ni une carte seule, et ne peut pas servir de parent ;
- la forme, la plage de profondeur, la taille, la teneur et la rareté ;
- une température maximale facultative (`max_temperature_c`) : la plage de profondeur d'un corps se resserre au-dessus de cet isotherme de sa colonne (même tirage aléatoire, sans effet sur le déterminisme). Elle sert de fenêtre à huile : 120 °C pour le pétrole, 200 °C pour le gaz ;
- à titre indicatif, les éléments obtenus après raffinage.

Le catalogue est **copié dans le monde** à la génération et son hachage entre dans l'empreinte : modifier le fichier ne change jamais un monde existant. Les blocs sont désignés **par leur nom**. Leurs identifiants `u16` sont attribués à la génération et enregistrés avec le monde (`blocks.toml`), en attendant le format de sauvegarde des blocs.

Formes, toutes déterministes et continues à travers la couture :

| Forme | Description | Hôte |
|---|---|---|
| `vein` (filon) | plaque mince et inclinée | le socle |
| `stock` (amas porphyrique) | ellipsoïde ; la teneur décroît du cœur vers le bord | le socle |
| `lens` (lentille) | petit ellipsoïde massif | le socle |
| `layer` (couche) | nappe horizontale à contour irrégulier, éventuellement avec un toit d'un autre bloc | les sédiments, ou le socle pour le fer rubané |
| `blanket` (manteau d'altération) | bande parallèle à la surface | les altérites |
| `placer` | bande allongée sous le lit d'une rivière ou sous une plage | sable et gravier |
| `trap` (piège) | carte seulement, sans bloc | — |

Placement : le monde est découpé en **tuiles de 2 km** (2 km divise 100 km). Chaque tuile a son propre RNG, dérivé de la graine, de l'étiquette `deposits` et de l'indice de la tuile. Ce RNG tire les corps selon les poids de contexte de la tuile.
- Un bloc de minerai ne remplace que de la roche (ou sable et gravier pour les placers), jamais l'air, l'eau, le sol ou le magma.
- Un corps qui recoupe la surface **affleure**.
- Les tailles restent à l'échelle du jeu (non divisées par 10).
- Rareté visée : environ 0,1 % du volume de roche des 500 premiers mètres sous un continent. Les métaux précieux sont environ dix fois plus rares que le fer, le charbon ou le sel.

Sorties : `deposits.bin` (les corps), couches `deposits` (gisement dominant) et `traps` (provinces et pièges des types « carte seulement », pétrole et gaz pour l'instant, au plus quatre types). `traps` est un champ de bits (province `1 << 2k`, piège `1 << (2k + 1)` du k-ième type) : les provinces de plusieurs types se superposent, la carte les montre en hachures alternées.

Le contexte dominant (`geo_context`) suit l'ordre ophiolite, point chaud, **orogène, puis arc** (un arc pris dans une collision est un orogène), rift, puis les contextes océaniques (dorsale, fosse, **marge passive**, océan) ou continentaux. Les abondances du catalogue sont calées sur la mesure : 0,11 % de blocs de minerai dans les 500 premiers mètres sous les continents de la graine 42.

**Profondeur et température** : aucun type ne doit se trouver exclusivement dans une roche à plus de 60 °C. L'outil `geology depths <monde>` mesure, pour chaque type, la profondeur du sommet et de la base des corps et la température de la roche à ces profondeurs, avec la croûte et la température de surface réelles de chaque colonne. Il signale les types dont la majorité des corps commencent au-delà de 60 °C.

Première liste (à affiner avec le gameplay) :

| Contexte tectonique ou climatique | Minerais (sous-produits) |
|---|---|
| Arc volcanique au-dessus d'une subduction | cuivre porphyrique (chalcopyrite), molybdène (molybdénite), or ; or filonien |
| Vieux cratons continentaux | fer rubané (magnétite, hématite) |
| Orogène de collision, granites | étain (cassitérite), tungstène (wolframite), or filonien |
| Ophiolites, roches ultrabasiques | chromite, nickel (pentlandite, avec cobalt), platine (avec palladium) |
| Bassins sédimentaires (à terre, sur les plateaux et sur les marges passives) | plomb-zinc (galène avec argent, sphalérite), charbon, sel gemme (avec potasse en toit), **pétrole et gaz (carte seulement)** |
| Plateaux marins et marges passives | manganèse (couche), phosphate (couche) |
| Latérites tropicales | bauxite (aluminium), nickel latéritique (garniérite, avec cobalt) sur les ophiolites ou les basaltes des points chauds |
| Rivières en aval des massifs riches | placers d'or et d'étain |
| Plages | placers côtiers d'ilménite (titane) |
| Points chauds, basaltes | soufre, obsidienne |

---

## 5. Génération des chunks (`nascio-voxel`)

`nascio-voxel` ne dépend que de `nascio-world` : la configuration, les grilles, le bruit, les fichiers du monde, le registre de blocs, les codes des couches, le catalogue et les corps de gisements, et le bruit de détail. La simulation (`nascio-worldgen`) en dépend aussi. Le terrain exige toutes les couches d'un monde complet : un monde antérieur à M5 est refusé avec le nom de la couche manquante.
- `generate_chunk(world: &WorldMacro, pos: ChunkPos) -> Chunk` est une fonction **pure et déterministe**.
- L'altitude de surface vient de l'interpolation bicubique périodique **monotone** de la grille de 32 m (passes d'Hermite à tangentes limitées : sans dépassement des nœuds voisins, continue), plus un bruit de détail périodique dont l'amplitude dépend du contexte (rugueux en montagne, doux en plaine).
- Le contenu vertical suit la colonne géologique du §4.6. L'eau va jusqu'à y = 0 pour les océans, et jusqu'au niveau des lacs et des rivières.
- **Rivières** *(config `[voxel]`, `river_*`)* :
  - **Réseau** : ordre de Strahler calculé sur tout le réseau de drainage des terres (`flow_direction`). De l'eau à partir de l'ordre `river_min_order` (4) ; en aval d'une cellule en eau, la rivière garde de l'eau jusqu'à son lac ou à la mer. Pas d'eau sur les cellules sous glace permanente (mois le plus chaud sous `glacier_warm_c`, ou biome glace) : une rivière naît au front de son glacier.
  - **Gabarit de jeu** (`river_gauge = "game"`, par défaut) : largeur et profondeur selon l'ordre de Strahler (ordres 4 à 8 et plus : 3, 6, 12, 24 et 40 m de large ; 1, 1,5, 2,5, 4 et 5 m de profondeur au centre). Les rivières n'ont pas à être réalistes pour le débit, mais elles doivent en avoir l'air et poser des défis (ponts, usage industriel). **Option** : le gabarit physique (`river_gauge = "physical"`) reprend la géométrie hydraulique des couches `river_width` et `river_depth` (eau à partir de `river_min_width_m`), qui donne moins de 1 m de profondeur dans un monde de 100 km.
  - **Surface** : elle part de la hauteur du sol de la cellule, ne remonte jamais vers l'aval et reste au moins au niveau du lac ou de la mer atteints ; elle est interpolée le long du tracé et arrondie au bloc (paliers sur les fortes pentes).
  - **Tracé** : des chaînes de cellules suivent le cours principal (ordre le plus élevé, puis le plus large). Les centres des cellules, lissés (`river_smooth_passes`), sont reliés par une spline de Catmull-Rom. Les **méandres** la déplacent latéralement selon une courbe « sine-generated » (Langbein : le cap oscille de ±`river_meander_angle_deg`, 70°, le long du cours), de longueur d'onde `river_meander_wavelength` (12) fois la largeur, avec une amplitude d'au plus `river_meander_max_amplitude` (3) largeurs ; ils s'effacent quand la pente moyenne sur une longueur d'onde passe de 1,5 % à 8 % (`river_meander_flat`, `river_meander_steep`). Un affluent aboutit exactement sur le cours principal ; ses propres méandres s'effacent sur sa dernière longueur d'onde.
  - **Profil** : dans le chenal, le sol est creusé selon un profil parabolique (au moins un bloc d'eau) ; les berges (`river_bank_m`, 2 m) sont au niveau de l'eau ; au-delà, le sol reste dans une pente de `river_valley_slope` (0,6) autour de ce niveau, jusqu'à `river_valley_height_m` (16 m) au-dessus ou au-dessous : ni gorge verticale, ni digue. Le lit et les berges sont en gravier.
  - **Embouchure** : le tracé s'arrête là où le sol est déjà sous la mer ou le lac.
  - Le maillage lointain (horizon) ne montre pas les rivières.
- **Végétation (provisoire)** : un seul modèle de sapin (tronc de 7 blocs, aiguilles en cône, 9 blocs de haut, 2 blocs de rayon) quel que soit le biome, et des cactus de 3 blocs sur le sable du désert chaud. Le monde est découpé en cellules de `plant_cell_m` (5 m) ; chaque cellule porte au plus une plante, dont la position et la présence sont tirées d'un hachage de la cellule et de la graine (étiquette `vegetation`) ; la densité et les modèles viennent de `[[voxel.vegetation]]` par biome. Pas de plante dans l'eau, sur les rivières et leurs berges, sur les glaciers, sur la roche nue ni au-delà de `plant_max_slope` ; les arbres seulement sur un sol (pas sur le sable ni le gravier) et sous la limite des arbres (mois le plus chaud ≥ `tree_line_warm_c`, 10 °C). Une plante peut déborder sur les chunks voisins ; le terrain l'emporte sur les aiguilles, le tronc sur les aiguilles d'un voisin. L'horizon ne montre pas la végétation. En hiver, la neige saisonnière (rendu) couvre le dessus des aiguilles mais laisse leurs côtés presque verts.
- **Limites de biomes** : le biome d'une colonne est lu à une position déformée (déformation des contextes, plus une déformation propre de 250 m de longueur d'onde), puis décalée au hasard d'au plus `biome_dither_m` (64 m) par un hachage de la colonne : les limites forment une frange mêlée d'environ 100 m pour les sols comme pour la densité des plantes.
- **Eau** : mêmes blocs pour la mer, les lacs et les rivières ; l'opacité de la surface croît avec la profondeur de la colonne (0,4 sur un bloc, 0,97 au large) et la teinte s'assombrit de même : les rivières sont claires, la mer devient vite opaque.
- **Glace et neige permanentes** *(config `[voxel]`)*, selon la température du mois le plus chaud ramenée à la hauteur du bloc par le gradient vertical :
  - **glacier** (biome glace, ou mois le plus chaud sous 0 °C) : bloc `ice` sur 12 m à plat et 2 m à la pente de la roche nue, sous 2 m de `snow` là où le mois le plus chaud reste sous −3 °C (zone d'accumulation) ; glace à nu au-dessus (zone d'ablation) ;
  - **surcouche de neige** (`snow_layer`, 1/8 de bloc, dans la cellule d'air au-dessus du sol) hors glacier, là où le mois le plus chaud reste sous 2 °C ;
  - **banquise** : 2 m de glace au sommet de l'eau (mer, lac, rivière) dont le mois le plus chaud reste sous 0,5 °C. La température de l'eau est celle de la cellule climatique, sans le relèvement dû aux terres de la cellule ; le seuil varie de ±`water_ice_dither_c` (1,5 °C) selon un bruit de longueur d'onde `water_ice_dither_m` (300 m) : bord ondulé au lieu d'une isotherme droite de la grille climatique.
  La neige saisonnière reste un effet de rendu jusqu'à la météo.
- **Bruit périodique** : bruit de gradient sur réseau, avec un espacement de réseau qui **divise 100 000** (16, 20, 25, 32, 40, 50, 80, 100, 125, 160, 200, 250, 400, 500, 625, 800, 1 000…). Le hachage des nœuds se fait modulo `100 000 / espacement`.
- Les identifiants de bloc sont des `u16`, avec un registre de blocs (nom, couleur provisoire, propriétés physiques de base). Les blocs sont désignés par leur nom ; les identifiants sont attribués à la génération du monde (blocs de la colonne, puis blocs du catalogue des gisements) et enregistrés dans `blocks.toml`.
- Les grottes et les structures viendront plus tard.
- **Modifications** : `Chunk = generate(pos) ⊕ delta(pos)` (`nascio-voxel::delta`). Un delta garde les blocs modifiés d'un chunk **en valeur absolue** (le bloc présent, pas une différence) : une modification garde son sens si le générateur change. Liste triée `(indice, bloc)` ; au-delà de `delta_palette_above` blocs (2 048), une palette (`KEEP` pour les blocs générés) sur 1, 2, 4, 8 ou 16 bits par bloc.
- **Fichiers** : `world-<seed>/deltas/r.<rx>.<rz>.bin`, une région de 25 × 25 colonnes de chunks (800 m), toutes hauteurs. En-tête : `NSCDELTA`, format (2), `generator_version`, empreinte de génération (`GenConfig::fingerprint` : `World::hash` et hachage de la config `[voxel]`, §6), **numéro de séquence (LSN) du dernier enregistrement du journal appliqué à la région**, puis les noms des blocs utilisés (les identifiants sont renumérotés au chargement). Des deltas d'un autre monde sont refusés ; ceux d'une autre version du générateur sont chargés avec un avertissement. Un fichier de format 1 (sans LSN) se lit avec un LSN nul. Écriture durable : fichier temporaire, `fsync`, renommage, `fsync` du dossier (Unix).
- **Journal** : un journal **global** en ajout seul, découpé en segments `world-<seed>/deltas/wal/w.<lsn>.log` (LSN du premier enregistrement, 16 chiffres hexadécimaux). En-tête d'un segment : `NSCWAL`, format, `generator_version`, empreinte. Enregistrement : longueur (`u32`), LSN (`u64`), type (`u8`), contenu, CRC-32C (sur LSN, type et contenu). Type 1 : édition de bloc (`x, y, z`, nom du bloc). Les autres types sont réservés aux états de jeu à venir (inventaires…), qui devront passer par le même journal pour qu'une action reste atomique.
- **Validation groupée** : une édition est écrite aussitôt dans le segment courant (le système l'a : elle survit à l'arrêt brutal du programme), puis appliquée en mémoire ; `flush` fait un seul `fsync` pour toutes les éditions depuis le précédent (elles survivent alors à une coupure de courant). Le serveur l'appelle à chaque intervalle `fsync_interval_ms` (50 ms, un tick). Après une coupure de courant, le monde revient en arrière d'au plus un intervalle, de façon cohérente (le journal est ordonné).
- **Points de reprise** : quand le journal dépasse `checkpoint_bytes` (16 Mio), un nouveau segment est ouvert, chaque région modifiée depuis le dernier point de reprise est réécrite (écriture durable, avec son LSN), puis les anciens segments sont supprimés. Une région déchargée de la mémoire alors qu'elle est modifiée est d'abord réécrite. Le visualiseur (mode dieu) appelle `flush` après chaque édition.
- **Chargement** : l'ouverture lit les en-têtes des régions et parcourt le journal une fois (index des enregistrements par région). Une région est chargée à sa première lecture : son fichier, puis ses enregistrements de LSN supérieur au sien. Un enregistrement incomplet ou de CRC faux à la fin du dernier segment est une écriture interrompue : il est retiré, avec tout ce qui le suit. Une erreur ailleurs est une corruption : l'ouverture échoue. Les anciens journaux par région (`r.<rx>.<rz>.log`) sont rejoués une fois, puis la région est réécrite et le journal supprimé.
- **Régions en mémoire** : une région illisible se lit comme non modifiée et refuse les éditions, sans toucher à ses fichiers. Chaque région a son propre verrou ; les lecteurs reçoivent un instantané par chunk, si bien qu'une édition copie au plus un chunk. Au plus `max_regions` régions (64) restent en mémoire, les moins récemment utilisées étant déchargées. Ces réglages (`DeltaConfig`) ne changent pas les chunks : ils ne font pas partie de l'empreinte.
- **Cache de colonnes** : `ColumnCache` garde au plus N `ColumnSet` (environ 100 Ko chacun avec plantes et corps minéralisés), la moins récemment utilisée étant retirée en premier.
- **Outils de mesure** (crate `nascio-tools`) : `chunkbench` mesure la génération et le maillage des chunks sur 1 et N cœurs (colonnes et chunks par seconde, temps CPU par étape) ; `topview` exporte des vues de dessus en PNG (un pixel par bloc : bloc le plus haut de chaque colonne, couleurs du visualiseur, eau mêlée à son lit selon la profondeur, ombrage du relief), sur des sites tirés au hasard (`--seed`), donnés (`--at x,z`) ou sur les rivières les plus profondes (`--rivers N`).

---

## 6. Format d'un monde sur disque
```
world-<seed>/
  world.toml          graine, version du générateur, paramètres, GAME_EPOCH
  layers/<nom>.bin    une couche par fichier : en-tête (largeur, hauteur, type, unité) + données brutes little-endian
  deposits.toml       copie du catalogue des gisements utilisé (§4.6)
  blocks.toml         registre des blocs : nom → identifiant u16, couleur, matière, densité
  deposits.bin        corps des gisements (forme, position, taille, orientation, teneur)
  previews/<nom>.png  aperçus
```
La version du générateur est incrémentée à chaque changement qui modifie le résultat. L'empreinte d'un monde (`hash`) couvre les couches, le catalogue, le registre des blocs et les corps des gisements.

**Empreinte de génération.** Les chunks dépendent du monde et de la configuration `[voxel]`. Cette configuration est celle du générateur (`VoxelConfig::default()`) : elle n'est pas stockée dans le monde, et la modifier incrémente `generator_version` (un test lie le hachage de la config par défaut à la version). L'empreinte de génération combine `World::hash` et le hachage de `[voxel]` ; les deltas y sont rattachés. Une autre configuration n'est acceptée qu'avec l'option de développement `--dev-override` (visualiseur, `topview`, `chunkbench`) : les deltas restent alors en mémoire et toute sortie est marquée `DEV-OVERRIDE` (titre de la fenêtre, noms des fichiers, lignes affichées).

**Déterminisme entre plateformes.** Le code de génération (`core`, `worldgen`, `voxel`) n'appelle pas les fonctions transcendantes de la plateforme (`sin`, `exp`, `powf`, `hypot`…, dont les derniers bits diffèrent entre la CRT de Windows et la glibc) mais `nascio_core::fmath::DetMath`, qui passe par la crate `libm` (portage en Rust pur de la libm de musl) : mêmes bits partout. `sqrt` est exact (IEEE 754) et `powi` se réduit à des multiplications. Une règle clippy (`disallowed-methods`) l'impose ; le rendu et les outils en sont exemptés. Des hachages de référence (monde de graine 42 au 1/25, 24 chunks dont les coutures, bits exacts de la hauteur de surface en 4 096 points) sont vérifiés par la CI sous Linux et sous Windows.

Pendant la génération, une **fenêtre de progression** (visualiseur, `--generate`) affiche le pourcentage total, l'étape en cours avec sa propre avancée et le temps écoulé. La CLI affiche la même chose en console. La progression ne fait que lire l'avancement : elle n'influe pas sur le résultat.

---

## 7. Visualiseur (`nascio-viewer`, Bevy + bevy_egui)
**Mode Carte (2D)**
- Affiche une couche choisie, avec sa légende et ses palettes. Zoom et déplacement.
- **Répétition périodique** (UV en `fract`) pour voir que la couture est continue.
- Graticule de latitude et de longitude : les lignes de latitude sont espacées selon `sin φ`.
- Curseurs de jour de l'année et d'heure, avec superposition **jour/nuit** calculée par pixel à partir de `e_vis`, et couches climatiques évaluées à cet instant.
- Infobulle sous le curseur : x, z, φ, λ et les valeurs des couches.
- **Coupe verticale** : un trait tracé sur la carte (clic droit, glisser) ouvre une coupe 2D du sous-sol le long de ce trait. Elle montre les roches de la colonne, le Moho, les isothermes du sous-sol et les gisements. L'outil `geology` de la crate `nascio-tools` exporte la même coupe en PNG (`cargo run -p nascio-tools --release --bin geology -- section <monde> --from x,z --to x,z --out f.png`) ; il mesure aussi les parts de roches et de minerais (`stats`), la profondeur et la température des gisements (`depths`) et trace les courbes de température du sous-sol (`geotherm`).
- Couches `geo_context`, `sediment_thickness`, `weathering_depth`, `laterite`, `placer_source`, `deposits` (gisement dominant) et `traps`, avec leurs légendes (celles des gisements viennent du catalogue copié dans le monde).
- Un clic choisit le **point d'apparition** (cellule surlignée) ; un bouton ou un double clic y passe en 3D.

**Mode 3D (vol libre)**
- Caméra libre (ZQSD/WASD et souris, vitesse ajustable de 5 m/s à 2 km/s), téléportation vers des coordonnées.
- Chargement des chunks autour de la caméra par génération asynchrone, maillage glouton (*greedy meshing*), couleurs par bloc (les textures viendront plus tard). Distance de vue réglable, 1 024 m par défaut.
- **Horizon** : au-delà des chunks, un maillage de hauteurs en anneaux emboîtés dont la résolution diminue de moitié à chaque anneau, sur plusieurs dizaines de km. La portée reste sous 50 km, pour ne jamais revoir le même relief par l'autre côté du tore. Les hauteurs viennent de la même fonction de surface que les chunks, avec des jupes entre niveaux. Aucune couture visible, ni avec les chunks ni à la couture du tore.
- Rendu relatif à la caméra. Traverser la couture ne doit produire aucun artefact.
- **Courbure d'affichage** (rendu uniquement ; la physique et la génération restent planes) : le shader de sommets des chunks, de l'eau et de l'horizon abaisse chaque sommet de `d² / (2·R_vis)`, où `d` est la distance horizontale à la caméra. `R_vis` *(config `[render] curvature_radius_m`, 200 km par défaut, curseur du panneau 3D ; 0 = plat)* ; le rayon réel, de même circonférence que le tore, vaut `100 km / 2π ≈ 15 915 m`. Les boîtes englobantes des chunks sont prolongées vers le bas de la chute maximale, l'horizon n'est pas soumis au frustum culling. Un point d'altitude `H` est visible depuis l'altitude `h` jusqu'à `√(2·R_vis·h) + √(2·R_vis·H)` : l'horizon ne dessine que les niveaux en deçà, avec `H` le plus haut sommet de l'horizon affiché.
- Soleil, ciel, couvert polaire, aurores, Lune et étoiles selon les §2.5 à §2.8. Le temps est contrôlable : temps réel, accéléré, ou figé sur une date et une heure.
- **Atmosphère** (rendu uniquement) : une seule fonction, partagée par le dôme du ciel et par les matériaux du relief (le bloc `atmosphere`, identique dans `sky.wesl` et `curvature.wesl`). Elle décrit une atmosphère exponentielle de hauteur d'échelle 8 km et d'épaisseur optique verticale 0,3 au-dessus du sol affiché, abaissé comme le relief. Un rayon au-dessus du limbe recueille l'air qu'il traverse depuis son point le plus bas ; un rayon sous le limbe finit sur le sol. Il n'y a plus de brouillard de distance : le relief reçoit la brume du même air, puis se fond près du bord de l'horizon dans la couleur que le dôme montre dans la même direction. Le relief, la courbure et le ciel se raccordent donc sans cassure. Le dôme et les étoiles passent par le même *tone mapping* que les matériaux éclairés. Pour garder le bleu du dôme de M4 malgré ce *tone mapping*, la couleur du ciel clair est saturée (×1,3 à l'horizon, ×1,9 au zénith) et éclaircie (×1,6) avant le mélange gris des nuages, dans la fonction d'air elle-même : la brume reste identique au ciel. Le sol vu au-delà du relief prend la couleur de la surface du monde (une texture périodique de 512², mêmes couleurs que l'horizon), éclairée par le soleil de la caméra ; elle s'efface entre 40 et 50 km vers un albédo moyen, pour ne jamais montrer deux fois le même lieu à travers le tore.
- **Selon l'altitude de la caméra** : le bleu s'éteint à mesure que la colonne d'air au-dessus s'amincit (environ la moitié de la luminosité du zénith à 5 km, un dixième à 20 km, le noir à 100 km). Les étoiles apparaissent en fondu de jour entre environ 10 et 30 km. Une fine bande d'atmosphère reste visible au limbe. La planète cache les étoiles, le Soleil et la Lune sous le limbe. On quitte la couche nuageuse entre 3 et 6 km. Le dôme et les étoiles grandissent avec l'altitude pour rester au-delà du relief, ce qui permet de monter jusqu'en orbite. Les étoiles sont des points ronds (profil radial sur chaque petit carré).
- Interface affichée : position, φ, λ, s, heure solaire locale, date du jeu, biome, roche sous le curseur. Champ de vision vertical de 42°. **Minimap** en bas à droite (position et cap, monde entier ou 20 km). État de la vsync et case pour la couper (Carte et 3D). Curseur de magnitude limite des étoiles (BSC5 jusqu'à la limite absolue de 6,5, 5 par défaut). Horloge en haut à droite : heure solaire locale et heure de l'origine (λ = 0), puis la température, l'humidité relative et la pression de l'air à la caméra (§4.3). **Sous terre**, ces trois valeurs sont remplacées par la profondeur, la roche et sa température (§4.6), avec une alerte dans le magma.
- Compteur à l'écran des temps CPU et GPU par image ; mode benchmark (trajet de caméra scripté, sortie en ms par image).
- **Cible** : 60 i/s en 1080p, réglages maximum, sur un portable Linux (Ryzen 5 5600H, RTX 3050 Mobile).

---

## 8. Réseau (`nascio-server`, `nascio-client`, `nascio-protocol`)

### 8.1 Architecture
- Un seul processus serveur, autoritaire, sans découpage du monde. Simulation à tick fixe de 20 Hz sur son propre fil ; réseau asynchrone sur d'autres fils. Les entrées des clients sont traitées par identifiant de joueur croissant.
- Les messages de contrôle sont encodés en postcard, dans des trames « longueur varint + contenu » ; `PROTOCOL_VERSION` vaut 6 et augmente à chaque changement de format (crate `nascio-protocol`, sans réseau). Le client (`nascio-client`) ne dépend que de `nascio-core`, `nascio-chunk` et `nascio-protocol`.
- Transport QUIC (TLS 1.3, rustls) : un flux bidirectionnel de contrôle, des flux unidirectionnels pour les chunks, des datagrammes non fiables pour les états des entités.

### 8.2 Poignée de main
Le client envoie la version du protocole, son nom de compte et une preuve d'identité, dans le canal chiffré (certificat du serveur épinglé) :
- **un jeton du site**, cas normal des joueurs. Le site, qui gère l'inscription et l'invitation, signe en Ed25519 `base64url(postcard(Claims) ‖ signature)`, où `Claims` = {version, nom, serveur visé (`server_id`), émission et expiration en ms UTC, nonce de 16 octets}. Le serveur ne connaît que la clé publique (`site_public_key`). Il vérifie, dans l'ordre :
  1. la signature ;
  2. la version, l'audience et le nom (celui du message) ;
  3. une validité d'au plus 5 min ;
  4. l'expiration et l'émission dans le futur, avec ±30 s de tolérance d'horloge ;
  5. une émission postérieure au démarrage du serveur + 30 s, car les nonces sont gardés en mémoire et un redémarrage ne doit pas ouvrir de fenêtre de rejeu ;
  6. un nonce jamais vu (usage unique).

  Un jeton valide crée le compte s'il n'existe pas ;
- **un secret**, réservé par défaut aux comptes administrateurs (`secret_login` = `admin`, `all` ou `none`). Le serveur le vérifie avec Argon2id et un sel par compte.

Ensuite, le serveur refuse un compte banni (bannissement par compte, temporaire ou permanent, avec sa raison, signalé seulement après une preuve valide) et une deuxième session. Il bloque une adresse après trop d'échecs. Enfin, il répond avec `generator_version`, l'empreinte de génération, la table des blocs (leurs définitions : identifiant, nom, couleur, matière, densité) et son heure UTC. Le client mesure le décalage de son horloge (aller-retour, à la manière de NTP) et dérive le temps du jeu de l'UTC corrigée : aucune horloge interne.

#### Format du jeton, octet par octet
Un jeton est la chaîne `base64url(charge ‖ signature)` : alphabet URL de la RFC 4648 §5 (`A–Z a–z 0–9 - _`), **sans** remplissage `=`, au plus 512 caractères. Le serveur refuse une forme non canonique, c'est-à-dire des bits de fin non nuls.

La **charge** enchaîne les champs suivants, dans cet ordre et sans séparateur (c'est l'encodage postcard de `Claims`) :

| Champ | Encodage |
|---|---|
| `version` | 1 octet, vaut `0x01` |
| `name` | longueur en varint, puis les octets UTF-8 (3 à 16 caractères parmi `A–Z a–z 0–9 _`) |
| `server` | longueur en varint, puis les octets UTF-8 (le `server_id` visé) |
| `issued_unix_ms` | entier signé de 64 bits en zigzag, puis en varint |
| `expires_unix_ms` | même encodage ; au plus `issued_unix_ms` + 300 000 |
| `nonce` | 16 octets bruts, aléatoires (pas de longueur) |

Définitions des encodages :
- **varint** (LEB128 non signé) : 7 bits par octet, poids faible d'abord ; le bit `0x80` signale qu'un octet suit.
- **zigzag** : `(v << 1) ^ (v >> 63)`, avec un décalage arithmétique, interprété comme un entier non signé de 64 bits.

La **signature** fait 64 octets. C'est une signature Ed25519 (RFC 8032, sans pré-hachage ni contexte) de la charge, telle quelle. Le serveur la reconnaît comme les 64 derniers octets ; tout le reste est la charge, qui doit se lire exactement, sans octet de trop.

**Vecteurs de test** (vérifiés par `crates/server/src/token.rs`, test `fixed_vectors`). Ed25519 est déterministe : une implémentation correcte doit retrouver exactement ces valeurs.

Entrées :
- graine de la clé privée (32 octets) : `000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f` ;
- `name = "alice"`, `server = "eu-1"`, `issued_unix_ms = 1 800 000 000 000`, `expires_unix_ms = 1 800 000 300 000` ;
- `nonce = a0 a1 a2 … af`.

Clé publique attendue :
- en hexadécimal : `03a107bff3ce10be1d70dd18e74bc09967e4d6309ba50d5f1ddc8664125531b8` ;
- en base64url, la forme de `site_public_key` : `A6EHv_POEL4dcN0Y50vAmWfk1jCbpQ1fHdyGZBJVMbg`.

Charge attendue (40 octets) :
```
01                                  version
05 616c696365                       "alice"
04 65752d31                         "eu-1"
80c0e285e368                        issued  (zigzag 3 600 000 000 000)
c08f8786e368                        expires (zigzag 3 600 000 600 000)
a0a1a2a3a4a5a6a7a8a9aaabacadaeaf    nonce
```

Signature attendue :
```
dcc27d3b4a7db6312a01d13fef72eb799d6d31bd823e03e11cdbdaa09de5ed04
7dd3a820d744721b2871441d61af9c7f0919e3f26684d7fc9ba5f53eab981808
```

Jeton attendu (139 caractères) :
```
AQVhbGljZQRldS0xgMDiheNowI-HhuNooKGio6SlpqeoqaqrrK2ur9zCfTtKfbYxKgHRP-9y63mdbTG9gj4D4Rzb2qCd5e0EfdOoINdEchsocUQdYa-cfwkZ4_JmhNf8m6X1PquYGAg
```

### 8.3 Chunks
- Codec v2 : un octet de version, la palette (nombre puis identifiants, varints), puis des plages (indice de palette, longueur − 1), varints, dans l'ordre des indices des blocs (`x`, puis `z`, puis `y`) ; pas de plages quand la palette n'a qu'une entrée : un chunk uniforme tient en 3 à 5 octets.
- Une colonne transporte aussi ses données de rendu, calculées par le serveur à partir du climat : bloc teinté, couleurs des quatre coins (`f32`), profondeur de l'eau de chaque colonne locale.
- Les chunks voyagent par lots compressés en zstd (niveau 3) ; l'unité de cache du serveur est la colonne de chunks encodée et compressée, faite une fois et partagée entre les clients, invalidée par une édition.
- Le serveur choisit les colonnes envoyées (rayon `chunk_radius_m`, 512 m par défaut ; colonnes les plus proches d'abord). Au-delà, l'horizon sera servi en tuiles de hauteurs précalculées (à venir). Le client ne génère jamais de terrain.
- Les éditions sont diffusées à la fin de chaque tick, par chunk (`Blocks` : indice local et bloc de chaque édition) ; une édition refusée est signalée au client (`EditRefused`), qui remet le bloc. Une édition n'est acceptée que dans une colonne à laquelle le joueur est abonné, et le joueur est abonné à une colonne avant que le serveur la lise.
- Une colonne porte une version : le LSN de sa région (§5) lu avant ses chunks. Un lot d'éditions porte le LSN de sa dernière édition ; le client l'applique à une colonne de version inférieure et le garde tant que la colonne n'est pas arrivée (colonnes et contrôle voyagent sur des flux différents). Au-dessus des chunks d'une colonne, il n'y a que de l'air.
- Le client pourra garder les colonnes reçues dans un cache disque, indexé par l'empreinte de génération, la position et la version (à venir).

### 8.4 Entités et débit
- Grille spatiale de 32 m ; pour chaque client, un accumulateur de priorité (distance, ancienneté du dernier envoi) ; les 64 entités les plus prioritaires à 20 Hz, les autres à 5 Hz au moins.
- Positions quantifiées (1/64 m ; lacet sur 16 bits, tangage sur 8 bits), envoyées en différence par rapport au dernier état confirmé par le client. Chaque datagramme porte un numéro (`seq`) et le tick du serveur ; le client acquitte les numéros reçus (`Ack`, flux de contrôle). Une entité en différence désigne le datagramme qui lui sert de base (au plus 64 datagrammes plus tôt) ; sinon elle est envoyée en entier. Une entité sortie du rayon (`entity_radius_m`, 256 m) est envoyée comme retirée jusqu'à l'acquittement du retrait.
- Le client affiche les entités 100 ms derrière le serveur, par interpolation sur le tore, avec une extrapolation de 250 ms au plus.
- Budget par client `client_budget_bytes_per_s` (128 Ko/s) : entités, puis éditions, puis chunks.

### 8.5 Limites et abus
- Chaque client a des seaux de jetons pour ses messages de contrôle et ses éditions ; au-delà, le message est ignoré ou l'édition refusée, et après trop de refus (`kick_after_violations`, 500) le client est expulsé. Exception : les positions (`Move`) et les acquittements (`Ack`) en excès sont jetés sans compter comme des refus, car un client renvoie d'un coup son arriéré après une coupure de sa liaison ; mais un épisode de dépassement qui dure plus de `overrun_kick_after_s` (10 s), sans pause d'une seconde, expulse le client.
- Une adresse est bloquée après `login_failures_per_min` (5) échecs de connexion par minute. Au plus `account_hash_concurrency` hachages Argon2id ont lieu à la fois.
- Un déplacement plus rapide que la vitesse maximale est refusé et le joueur est remis à sa dernière position acceptée (`Correct`) ; une édition hors de portée est refusée. Les valeurs (vitesse, portée) relèvent du gameplay.
- Les refus sont consignés dans `<data>/server/abuse.log` (heure, adresse, compte, type, détail), sans jamais de secret ; le premier événement de chaque type est écrit, puis un sur cent.
- Le serveur refuse de démarrer avec `server_id = "dev"` (ou vide) sur une adresse non locale ; il écoute par défaut sur `127.0.0.1:7777`.
