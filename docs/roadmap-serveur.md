# Feuille de route — Phase « Serveur et réseau »

Plan présenté le 2026-11-18 ; travail de nuit autonome demandé par l'auteur le même jour (étapes dans l'ordre, un rapport et un commit par étape dans `docs/reports/`, pas de push, spec inchangée : texte proposé dans `docs/reports/spec-proposee.md`).

Hypothèses de travail (questions du plan restées ouvertes, à confirmer par l'auteur) :
- hébergement Linux, une machine de 8 à 16 cœurs, 1 Gbit/s en sortie (100 Mbit/s garantis au moins) ;
- fenêtre de perte des éditions : un tick (50 ms) ;
- cible de charge : 500 joueurs dans un rayon de 200 m, 2 000 inscrits ;
- horizon servi en tuiles précalculées par le serveur (le client ne génère rien) ;
- client construit à partir du visualiseur, rendu partagé dans une bibliothèque.

## C0 — Correction de la compaction
Le fichier de région est synchronisé (`fsync`) avant le renommage, le dossier après (Unix), et le journal n'est supprimé qu'ensuite. **Validation** : tests verts, ordre des opérations testé.

## S0 — Mesures
Tailles et vitesses des codecs de chunks sur le monde de référence (v1, plages varint, palette compactée par bits, avec ou sans compression générique) ; coût du `fsync` seul ou groupé ; modèle de débit d'une bataille ; texte proposé pour la spec §7 « Réseau ». **Validation** : rapport chiffré, décisions consignées.

## S1 — Durabilité des deltas
Journal global en ajout seul (WAL) générique, validation groupée (un `fsync` par intervalle), LSN et CRC par enregistrement, points de reprise synchronisés dans les fichiers de région, segments du journal supprimés une fois couverts. Tests de crash (troncature, enregistrement déchiré, arrêt brutal). **Validation** : aucune perte au-delà d'un intervalle ; débit d'éditions mesuré.

## S2 — Protocole
Crate `nascio-protocol` : codec de chunks v2, messages, poignée de main (version, `generator_version`, empreinte, table des blocs, heure UTC), tests d'aller-retour `proptest`. **Validation** : tailles conformes à S0, tests verts.

## S3 — Serveur minimal
Crate `nascio-server` : QUIC, tick de 20 Hz, cache des chunks encodés partagé, diffusion par intérêt avec budget, éditions avec autorité et journal. **Validation** : un client et un bot voient la même édition.

## S4 — Client
Rendu extrait du visualiseur, client réseau, tuiles d'horizon, cache disque. **Validation** : même rendu que le visualiseur.

## S5 — Entités
États répliqués (priorité, quantification, différences), prédiction et interpolation, collision minimale. **Validation** : mouvement fluide jusqu'à 150 ms de latence.

## S6 — Charge
Bin `swarm` : 500 bots dans un rayon de 200 m. **Validation** : p99 du tick < 50 ms, débit par client ≤ budget.

## S7 — Comptes et sécurité
Comptes (argon2), limites de débit, contrôles de portée et de vitesse, console d'administration. **Validation** : abus refusés et journalisés.
