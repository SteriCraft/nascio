# Feuille de route — Phase « Monde »

Chaque jalon se termine par une revue avec l'auteur. On ne passe pas au jalon suivant sans validation.

## M0 — Fondations
- Workspace Cargo (`core`, `worldgen`, `voxel`, `viewer`), `.gitignore` (`target/`, `out/`), formatage et clippy.
- CI GitHub Actions : fmt, clippy et tests sous Windows et Linux.
- `nascio-core` :
  - `torus` : wrap, delta, distance, versions i32 et f64 ;
  - coordonnées de bloc et de chunk ;
  - temps du jeu ;
  - astronomie (φ, λ, δ, δ_eff, e, e_vis, azimut, Q).
- Tests :
  - propriétés sur la couture ;
  - continuité de `e` et de `e_vis` à travers la couture ;
  - garanties de la bande crépusculaire (§2.5).

**Validation** : `cargo test` passe au vert ; un petit CLI ou un test affiche les courbes de hauteur du soleil sur 24 h pour plusieurs valeurs de s.

## M1 — Tectonique et visualiseur 2D
- Tectonique (§4.1), points chauds compris, à pleine et à basse résolution.
- Export des couches en `.bin` et en PNG.
- Visualiseur, mode Carte : sélection de couche, répétition périodique, graticule, infobulle.
- Test de déterminisme (même graine, même hachage).

**Validation** : plusieurs graines donnent des continents variés, des chaînes de montagnes aux collisions, des dorsales, des fosses, des archipels de points chauds, et une couture invisible.

## M2 — Premier terrain 3D et horizon
- Registre de blocs minimal (`u16`, couleurs provisoires) et `generate_chunk` pure et déterministe. L'altitude vient de la bicubique périodique de la grille tectonique de 160 m (provisoire jusqu'à M3), plus un bruit de détail périodique selon le contexte. Colonne simple : herbe, terre, pierre, sable, eau jusqu'à y = 0. L'interface `generate ⊕ delta` est déclarée, sans implémentation.
- Visualiseur, mode 3D :
  - caméra libre (5 m/s à 2 km/s) et téléportation ;
  - génération asynchrone, maillage glouton, rendu relatif à la caméra, éclairage simple ;
  - affichage de la position, de φ, λ et s ;
  - distance de vue de 512 m par défaut.
- Horizon : niveaux de détail en maillage de hauteurs au-delà des chunks, sur plusieurs dizaines de km.
- Compteur des temps CPU et GPU par image, et mode benchmark scripté.

**Validation** :
- on traverse les coutures est-ouest et nord-sud sans artefact, ni dans les chunks ni dans l'horizon ;
- aucune couture visible entre les chunks et l'horizon ;
- 60 i/s en 1080p, réglages maximum, sur le portable de référence (benchmark).

**Mesure de performance (2026-11-15)** : critère atteint sur le portable de référence (RTX 3050 Mobile, Linux) avec `bench/default.toml`. Sur 32 545 images, la moyenne est de 3,84 ms (260 i/s), le p95 de 5,92 ms, le p99 de 7,16 ms et le 1 % low de 116 i/s. Seules 2 images dépassent 16,7 ms (max 25,9 ms). La traversée à 2 km/s est limitée par le CPU (chargement des chunks, GPU sous 1 ms). Le passage le long de la haute chaîne est la phase la plus lourde pour le GPU (3,7 ms en moyenne).

## M3 — Érosion et hydrographie
- Érosion de type stream power sur la grille de 32 m, priority-flood, lacs, première ébauche de rivières.

**Validation** : les vallées et les bassins versants sont crédibles, et les rivières atteignent la mer ou un lac.

**Notes issues de M1** :
- la ligne de lacs en pointillés de la graine 7 (chenal de rift dans le grand continent) : à transformer en vrais lacs ou en vallée avec l'hydrologie ;
- la tranche −200..0 m presque vide dans l'histogramme hypsométrique : le plateau, très étroit (1,2 km en 1/5), et le premier anneau du talus laissent un creux ; à reprendre avec l'érosion et la sédimentation côtière.

**Réalisé (2026-11-16, à valider en revue)** :
- grille fine de 32 m (3 125², ou 625² en 1/5) : suréchantillonnage bicubique, bruit de détail selon la rugosité, rifts en graben continu ;
- précipitations provisoires (zonales, orographiques le long des vents zonaux, bande polaire) ;
- 1 Ma d'érosion : stream power implicite, diffusion de Roering, cônes de sédiments côtiers ;
- lacs (dépressions d'au moins 2 m et 20 000 m²), directions D8, débits, bassins par exutoire, rivières (débit ≥ 0,1 m³/s, ordre de Strahler) ;
- chunks et horizon sur la grille érodée, avec l'eau des lacs ;
- graines 42, 7, 1234 et 2026 : génération complète en pleine résolution en 48 à 60 s (érosion 41 à 53 s), 70 à 115 lacs par monde, terre abaissée de 37 à 68 m en moyenne ;
- ligne de lacs de la graine 7 : devenue une vallée de graben avec des lacs ; creux −200..0 m : comblé par les cônes de dépôt (voir `hypsometry.png`).
- benchmark `bench/default.toml` sur la graine 42 érodée (RTX 5060, Windows) : p95 de 4,29 ms, moyenne de 2,96 ms (338 i/s), 1 % low à 144 i/s ; aucune régression par rapport à M2. À refaire sur le portable de référence.

## M4 — Climat, saisons, biomes, ciel
- Climat (§4.3) avec stockage en harmoniques, hydrologie finale (§4.4), biomes (§4.5).
- Chunks générés jusqu'au Moho (pierre générique), puis magme du manteau ; la vraie colonne de roches vient en M5.
- Visualiseur : couches climatiques animées selon la date, superposition jour/nuit.
- Ciel 3D : course du soleil, crépuscule et couvert nuageux de la couture, anneau d'aurores, Lune et étoiles, neige saisonnière en surface.

**Validation** : des cartes de température et de pluie plausibles en janvier et en juillet, des déserts subtropicaux, des forêts équatoriales, une sécheresse visible à l'abri des montagnes, une bande polaire sombre sans saut du soleil.

**Réalisé (2026-11-16, à valider en revue)** :
- revue de M3 : soulèvement tectonique pendant l'érosion, tiré des vitesses de convergence (sutures soudées comprises) ;
- climat sur la grille de 160 m (24 pas par an, harmoniques) : température (profil calé, inertie terre/mer, continentalité, gradient vertical, courants), vents à trois cellules suivant `δ_eff`, humidité advectée et pluies orographiques, ZCIT et ceintures sèches, nuages, neige au sol, bande polaire climatique (pluies fortes, neige permanente) ; amplitude diurne ;
- hydrologie finale avec les pluies du climat (largeur et profondeur des rivières) ; biomes Köppen sur la grille de 32 m (14 codes) ;
- chunks jusqu'au Moho (pierre générique), magma dessous ;
- côtes en escalier de 160 m corrigées (distorsion de domaine) ;
- visualiseur : cartes climatiques animées selon la date, jour/nuit par pixel ; en 3D, soleil calculé par pixel, ombres dans le shader courbé, dôme du ciel, Lune et ses phases, étoiles réelles, aurores, couvert polaire, neige saisonnière ; `R_vis` = 200 km ;
- graines 42, 7, 1234 et 2026 : génération complète en 61 à 73 s (climat 7,5 s) ; benchmark : p95 6,22 ms avec les ombres, 4,43 ms sans (M3 : 4,29 ms).

**Retouches après la revue de M4 (2026-11-16)** :
- relief marin : abysse à −1 800 m, fosses à −2 500 m, talus de 5 km sur toutes les marges, pente du fond bornée à 0,35 et pente côtière à terre bornée à 0,8 (plus de parois ni de murs de cônes) ; écart à l'hypsométrie terrestre consigné au §3 ;
- continentalité sur 20 km avec une saison amortie en mer ; soulèvement maximal de 0,5 mm/an ;
- rendu : neige sur les faces verticales et limite des neiges par l'altitude fine, étoiles du BSC5 jusqu'à la magnitude 6,5 (curseur, 5 par défaut), Lune texturée (NASA SVS), FOV de 42°, minimap, point d'apparition choisi sur la carte, vsync, horloge locale et d'origine, distance des chunks de 1 024 m par défaut ;
- graines 42, 7 et 1234 : génération en 80 à 93 s ; benchmark sur la graine 42 : moyenne 3,52 ms, p95 4,78 ms (M4 : 3,72 et 6,22 ms).

**Atmosphère et pluies par saturation (2026-11-16)** :
- pression atmosphérique selon le profil terrestre comprimé ×2, nulle à 50 km (472 hPa à 3 km) ; gradient thermique conservé à −6,5 °C/km ;
- humidité de l'air (rapport de mélange) advectée et exportée ; pluies par condensation au-delà de la saturation `w_s(T, P)` ;
- profil thermique réchauffé vers 45–60° : juillet de 14 à 17 °C sur les côtes, mer libre jusqu'à 55° ;
- plateformes de dépôt en pente depuis la côte : de −1 m au rivage jusqu'à −20 m environ à 1 km ;
- HUD 3D : température, humidité relative et pression à la caméra, sous l'horloge ;
- graines 42, 7, 1234 et 2026 : génération en 87 à 136 s (`generator_version` 8) ; benchmark sur la graine 42 : moyenne 3,49 ms, p95 4,77 ms (sans changement).

## M5 — Géologie et sous-sol
- Colonne géologique (§4.6), Moho variable, température en profondeur, magme mortel, gisements par contexte.
- Visualiseur : coupe verticale (2D) et couche des « gisements ».

**Validation** : une coupe à travers une zone de subduction et à travers un orogène montre les bonnes roches, la bonne profondeur du Moho et les bons gisements.

## M6 — Finition du générateur de chunks
- Biomes et climat dans les chunks (sol, végétation provisoire, neige, glace), eau des rivières et des lacs.
- Préparation pour le serveur : implémentation du stockage des deltas (`generate ⊕ delta`), mesures de performance (chunks par seconde).

**Plan validé (2026-11-17)**. Une étape à la fois ; à la fin de chaque étape, un court rapport, puis l'auteur vérifie dans le visualiseur. Les vérifications approfondies se font à la fin du jalon et à l'audit.
1. **Sol selon le biome** — fait. L'herbe est un seul bloc partout : sa couleur varie selon le climat, avec des transitions en dégradé (même une fois texturée).
2. **Neige et glace permanentes** — fait : bloc `ice` (teinté de bleu) pour les glaciers et la banquise (mer et lacs) ; bloc `snow` (blanc) pour la neige permanente ; une **surcouche de neige** fine qui peut s'accumuler (à la manière de Minecraft). La neige saisonnière reste un effet de rendu jusqu'à la météo.
3. **Eau des rivières** — fait : tracé lissé le long de `flow_direction` (distances par `torus::delta`), chenal creusé (largeur et profondeur des couches), surface de l'eau qui ne remonte jamais vers l'aval, paliers sur les fortes pentes, raccord avec les lacs et la mer. De l'eau à partir d'environ 1,5 m de large ; vers l'aval, un ruisseau garde au moins la plus petite largeur atteinte jusqu'à son lac ou à la mer. Mêmes blocs d'eau que la mer, transparents et teintés de bleu, plus sombres et plus opaques avec la profondeur. Pas d'animation ni d'écoulement.
4. **Végétation provisoire** — fait : un seul modèle d'arbre fixe, de type « sapin », quel que soit le biome ; des cactus de 3 blocs dans le désert chaud. Points de départ des troncs procéduraux et déterministes (hachage des coordonnées du monde), densité selon le biome ; un arbre peut déborder sur le chunk voisin. Une liste d'arbres par biome est prévue, à remplir plus tard (étape « Végétation » dédiée).
5. **Stockage des deltas** — fait : `DeltaStore` (blocs modifiés en valeur absolue, structure triée, palette au-delà d'un seuil), fichiers de région dans `world-<seed>/deltas/` avec la version du générateur et l'empreinte du monde ; outil de pose et de destruction dans le visualiseur (futur « god mode »).
6. **Performance** : binaire `chunkbench` (génération, maillage, 1 et N cœurs). Pas de cible fixée.
7. **Vérification et clôture** : vues de dessus en PNG, captures, `generator_version`, spec, décisions.

## Audit — Revue complète avant la phase « Serveur et réseau »
Audit complet du projet par Fable, à partir des derniers benchmarks du PC Windows (RTX 5060) et du portable Linux de référence (RTX 3050 Mobile). Il couvre :
- le déterminisme et le respect des invariants de `CLAUDE.md` (tore, coordonnées entières, RNG, bruit périodique) ;
- la performance de la génération et du maillage des chunks ;
- l'architecture des crates et le sens des dépendances ;
- la dette des solutions provisoires consignées dans `decisions.md` ;
- le rendu (shaders, éclairage, ciel, horizon).

**Validation** : un rapport priorisé, trié en revue avec l'auteur avant d'ouvrir la phase « Serveur et réseau ».

---
Ensuite viendra la **Phase « Serveur et réseau »**, qui fera l'objet d'une feuille de route séparée.
