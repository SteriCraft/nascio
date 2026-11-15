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
  - l'intensité de la lumière directionnelle est une fonction continue de `e_vis`, nulle pour `e_vis ≤ 0` (fondu entre 0 et +3°). **Elle est donc nulle dans la bande : aucune ombre** ;
  - la couleur du ciel et la lumière ambiante sont des fonctions continues de `e_vis` (crépuscule civil, nautique, nuit) ;
  - les **aurores** ont une intensité égale à `(1 − smoothstep(S_DARK, S_LIGHT + 1 000, s)) × facteur_nuit`.
- Le **climat** utilise `e` et `δ_eff` (insolation physique), pas `e_vis`. Près de la couture, l'insolation tend de toute façon vers 0.

### 2.6 Insolation journalière moyenne (climat)
`Q = (S0/π) · (h0 · sin φ · sin δ_eff + cos φ · cos δ_eff · sin h0)`, avec `h0 = arccos(clamp(−tan φ · tan δ_eff, −1, 1))` et `S0 = 1361 W/m²`.

---

## 3. Échelle verticale (cibles)
Le monde est environ 400 fois plus petit que la Terre à l'horizontale. Les profondeurs de croûte sont donc réduites d'environ 1/10, tandis que le relief garde des valeurs crédibles.

| Grandeur | Cible |
|---|---|
| Niveau de la mer | y = 0 |
| Plus hauts sommets | ~ +3 000 m |
| Dorsales océaniques | ~ −1 500 m |
| Plaines abyssales | ~ −3 000 m |
| Fosses de subduction | jusqu'à ~ −3 500 m |
| Moho sous l'océan | ~ 700 m sous le plancher |
| Moho sous un continent | ~ 3 500 m sous la surface |
| Moho sous une chaîne de montagnes | jusqu'à ~ 6 000 m sous la surface |
| Au-delà du Moho | **magme du manteau**, infini et mortel |

L'isostasie (modèle d'Airy) guide la relation entre l'épaisseur de la croûte et l'altitude, mais avec des paramètres **ajustés** pour atteindre ces cibles. On ne cherche pas l'exactitude physique.

---

## 4. Pipeline de génération macro (`nascio-worldgen`)
Ordre : **tectonique → érosion → climat → hydrologie finale → biomes → géologie**.
Chaque étape lit les couches précédentes et écrit les siennes. Chaque couche peut être exportée en PNG.

### 4.1 Tectonique (grille de 625², pas de temps fixes)
Référence : Cortial et al., *Procedural Tectonic Planets* (2019), adaptée à une grille torique.
- **Plaques** : de 8 à 12 *(config)*. On les initialise par un Voronoï périodique déformé par du bruit. Chaque plaque reçoit une vitesse de translation, et éventuellement une petite rotation.
- **Croûte initiale** : environ 30 % de croûte continentale *(config)*, regroupée en quelques masses ; le reste est océanique.
- **État par cellule** : id de plaque, type de croûte (océanique ou continentale), épaisseur, âge de la croûte océanique, altitude, drapeaux d'histoire (orogenèse, arc volcanique, rift, point chaud, ophiolite).
- **À chaque pas** : advection semi-lagrangienne des plaques (périodique), puis traitement des frontières :
  - **divergence** : création de croûte océanique d'âge 0 (dorsale). Sur un continent, création d'un rift ;
  - **subduction océan → continent** : la croûte océanique est consommée, une fosse se forme, et un **arc volcanique** apparaît sur la plaque chevauchante ;
  - **subduction océan → océan** : arc insulaire ;
  - **collision continent ↔ continent** : épaississement de la croûte et soulèvement (orogenèse), avec suture possible d'ophiolites ;
  - **frontière transformante** : simple cisaillement.
- **Subsidence océanique** : la profondeur croît comme `√âge`.
- **Points chauds** : de 2 à 5 *(config)*, fixes dans le repère du manteau. La plaque qui défile au-dessus reçoit une suite d'édifices volcaniques dont l'âge augmente avec la distance, ce qui donne des **archipels de type Hawaï**. Ils peuvent aussi tomber sous un continent (trapps, plateaux).
- Le nombre de pas suffit à faire émerger des continents de formes variées. On n'a pas besoin de cycles de Wilson complets.
- **Sorties** : `plate_id`, `crust_type`, `crust_thickness`, `crust_age`, `elevation_tectonic`, `moho_depth`, `history_flags`, `hotspot_track`.

### 4.2 Érosion (grille de 3 125²)
- Suréchantillonnage de l'altitude tectonique vers la grille de 32 m, avec du bruit périodique de détail.
- Érosion fluviale de type **stream power** (`E = K · A^m · S^n`), plus de la diffusion sur les pentes. Les précipitations sont d'abord estimées grossièrement (zonales et orographiques), car le climat n'est pas encore connu.
- Écoulement **D8 ou MFD périodique**. Les dépressions sont traitées par *priority-flood* (comblement ou lacs).

### 4.3 Climat (grille de 625²)
On simule une année type sur 24 à 48 pas, puis on **projette chaque grandeur sur des harmoniques annuelles** :
`X(N) = X0 + A1·cos(2π(N − p1)/365) + A2·cos(4π(N − p2)/365)`.
On stocke `(X0, A1, p1, A2, p2)` par cellule. L'évaluation à n'importe quel instant est **continue par construction**, ce qui garantit des transitions fluides entre les mois.
- **Température** : bilan radiatif simplifié à partir de Q, puis :
  - inertie thermique : déphasage d'environ 1 mois sur terre et 1,5 à 2 mois en mer, amplitude réduite en mer, continentalité selon la distance à la côte ;
  - gradient vertical de −6,5 °C/km ;
  - anomalies dues aux courants océaniques ;
  - calibration pour obtenir des moyennes zonales terrestres plausibles (≈ 26 °C à l'équateur, ≈ −20 °C en moyenne près de la couture).
- **Vents dominants** : trois cellules par hémisphère, définies **en latitude φ** (alizés pour 0–30°, vents d'ouest pour 30–60°, vents polaires d'est pour 60–90°). Les bandes se déplacent avec δ_eff selon la saison. La couture est un anticyclone polaire : les deux côtés divergent, ce qui est cohérent.
- **Courants océaniques** : gyres simplifiés entraînés par les vents et déviés par les côtes. On advecte des anomalies chaudes et froides : courants chauds sur les bords ouest des bassins, remontées froides sur les bords est en zone subtropicale.
- **Humidité et précipitations** : évaporation sur l'océan, advection par le vent, pluies orographiques au vent des reliefs et sécheresse à l'abri des montagnes, ZCIT (zone de convergence intertropicale) qui suit δ_eff, ceintures sèches vers 30°.
- **Cycle diurne** (à l'exécution) : son amplitude dépend de l'humidité et de la continentalité. Le maximum est vers 14–15 h solaires locales.
- **Sorties** : harmoniques de `temperature`, `precipitation`, `wind` (vecteur), `snow_fraction` ; champ `ocean_current`.

### 4.4 Hydrologie finale (grille de 3 125²)
On recalcule les débits avec les précipitations du climat, puis on détermine les rivières (seuil d'accumulation, largeur et profondeur selon le débit), les lacs et les deltas. Une légère passe d'érosion de finition est possible.

### 4.5 Biomes
Classification à partir des harmoniques de température et de précipitations, de type **Köppen** simplifié, ou Whittaker en premier jet. On ajoute des cas particuliers : glaciers et calottes (température moyenne annuelle et altitude), mangroves, marais.

### 4.6 Géologie et ressources
**Colonne verticale** (de haut en bas), calculée à partir des couches macro :
1. **Sol** de 0 à 3 m, selon le biome et la pente ;
2. **Altérites et régolithe** : latérite en climat tropical humide ;
3. **Couverture sédimentaire** : épaisseur selon les bassins (subsidence, deltas, marges passives), alternance grès, argile, calcaire, évaporites, charbon selon le paléo-contexte ;
4. **Socle** :
   - continental : granite et gneiss, ou schistes et roches métamorphiques dans les orogènes ;
   - océanique : basalte en surface, puis gabbro ;
   - arcs et points chauds : andésite et basalte ;
5. **Croûte inférieure** (dernier tiers environ avant le Moho) : granulite ou gabbro ;
6. **Moho**, puis **magme**, infini et mortel.

**Température du sous-sol** : `T(prof) = T_surface + (T_moho − T_surface) · (prof / moho_depth)`, avec `T_moho ≈ 1 200 °C` *(config)*. Le gradient est donc plus fort sous les océans, ce qui est cohérent avec un flux de chaleur plus élevé.

**Gisements** (première liste, à affiner plus tard avec le gameplay) :
| Contexte tectonique ou climatique | Minerais |
|---|---|
| Arc volcanique au-dessus d'une subduction | cuivre porphyrique (chalcopyrite), molybdène, or |
| Vieux cratons continentaux | fer rubané (magnétite, hématite) |
| Orogène de collision, granites | étain (cassitérite), tungstène, or filonien |
| Ophiolites, roches ultrabasiques | chromite, nickel, platine |
| Bassins sédimentaires | plomb-zinc (galène, sphalérite), charbon, pétrole, gaz, sel |
| Latérites tropicales | bauxite (aluminium), nickel latéritique |
| Rivières en aval des massifs riches | placers d'or et d'étain |
| Points chauds, basaltes | soufre, basalte, obsidienne |

Les gisements sont placés comme des **corps** cohérents (filons, amas, couches) par une génération déterministe propre à chaque région. Ils ne sont pas répartis en bruit uniforme.

---

## 5. Génération des chunks (`nascio-voxel`)
- `generate_chunk(world: &WorldMacro, pos: ChunkPos) -> Chunk` est une fonction **pure et déterministe**.
- L'altitude de surface vient de l'interpolation bicubique périodique de la grille de 32 m, plus un bruit de détail périodique dont l'amplitude dépend du contexte (rugueux en montagne, doux en plaine).
- Le contenu vertical suit la colonne géologique du §4.6. L'eau va jusqu'à y = 0 pour les océans, et jusqu'au niveau des lacs et des rivières. Glace et neige dépendent du climat.
- **Bruit périodique** : bruit de gradient sur réseau, avec un espacement de réseau qui **divise 100 000** (16, 20, 25, 32, 40, 50, 80, 100, 125, 160, 200, 250, 400, 500, 625, 800, 1 000…). Le hachage des nœuds se fait modulo `100 000 / espacement`.
- Les identifiants de bloc sont des `u16`, avec un registre de blocs (nom, couleur provisoire, propriétés physiques de base).
- Les grottes, les structures et le stockage des modifications des joueurs viendront plus tard. On prévoit seulement l'interface : `Chunk = generate(pos) ⊕ delta(pos)`.

---

## 6. Format d'un monde sur disque
```
world-<seed>/
  world.toml          graine, version du générateur, paramètres, GAME_EPOCH
  layers/<nom>.bin    une couche par fichier : en-tête (largeur, hauteur, type, unité) + données brutes little-endian
  previews/<nom>.png  aperçus
```
La version du générateur est incrémentée à chaque changement qui modifie le résultat.

---

## 7. Visualiseur (`nascio-viewer`, Bevy + bevy_egui)
**Mode Carte (2D)**
- Affiche une couche choisie, avec sa légende et ses palettes. Zoom et déplacement.
- **Répétition périodique** (UV en `fract`) pour voir que la couture est continue.
- Graticule de latitude et de longitude : les lignes de latitude sont espacées selon `sin φ`.
- Curseurs de jour de l'année et d'heure, avec superposition **jour/nuit** calculée par pixel à partir de `e_vis`, et couches climatiques évaluées à cet instant.
- Infobulle sous le curseur : x, z, φ, λ et les valeurs des couches.

**Mode 3D (vol libre)**
- Caméra libre (ZQSD/WASD et souris, vitesse ajustable de 5 m/s à 2 km/s), téléportation vers des coordonnées.
- Chargement des chunks autour de la caméra par génération asynchrone, maillage glouton (*greedy meshing*), couleurs par bloc (les textures viendront plus tard), et niveaux de détail simples pour l'horizon dans un second temps.
- Rendu relatif à la caméra. Traverser la couture ne doit produire aucun artefact.
- Soleil, ciel et aurores selon le §2.5. Le temps est contrôlable : temps réel, accéléré, ou figé sur une date et une heure.
- Interface affichée : position, φ, λ, s, heure solaire locale, date du jeu, biome, roche sous le curseur.
- Cible : 60 i/s sur une RTX 5060.
