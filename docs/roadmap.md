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

## M2 — Premier terrain 3D
- Registre de blocs minimal et `generate_chunk` à partir de l'altitude tectonique, avec du bruit de détail périodique.
- Visualiseur, mode 3D : caméra libre, chargement des chunks, greedy meshing, éclairage simple.

**Validation** : on se promène sur le relief, on traverse les coutures est-ouest et nord-sud sans artefact, à 60 i/s.

## M3 — Érosion et hydrographie
- Érosion de type stream power sur la grille de 32 m, priority-flood, lacs, première ébauche de rivières.

**Validation** : les vallées et les bassins versants sont crédibles, et les rivières atteignent la mer ou un lac.

## M4 — Climat, saisons, biomes, ciel
- Climat (§4.3) avec stockage en harmoniques, hydrologie finale (§4.4), biomes (§4.5).
- Visualiseur : couches climatiques animées selon la date, superposition jour/nuit.
- Ciel 3D : course du soleil, crépuscule de la couture, aurores, neige saisonnière en surface.

**Validation** : des cartes de température et de pluie plausibles en janvier et en juillet, des déserts subtropicaux, des forêts équatoriales, une sécheresse visible à l'abri des montagnes, une bande polaire sombre sans saut du soleil.

## M5 — Géologie et sous-sol
- Colonne géologique (§4.6), Moho variable, température en profondeur, magme mortel, gisements par contexte.
- Visualiseur : coupe verticale (2D) et couche des « gisements ».

**Validation** : une coupe à travers une zone de subduction et à travers un orogène montre les bonnes roches, la bonne profondeur du Moho et les bons gisements.

## M6 — Finition du générateur de chunks
- Biomes et climat dans les chunks (sol, végétation provisoire, neige, glace), eau des rivières et des lacs, niveaux de détail à l'horizon.
- Préparation pour le serveur : interface `generate ⊕ delta`, mesures de performance (chunks par seconde).

---
Ensuite viendra la **Phase « Serveur et réseau »**, qui fera l'objet d'une feuille de route séparée.
