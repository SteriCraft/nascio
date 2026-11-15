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
