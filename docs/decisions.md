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
