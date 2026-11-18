# État du travail — Phase « Serveur et réseau »

À relire après chaque compactage, avec `CLAUDE.md` et `docs/roadmap-serveur.md`.

**Consignes de la nuit (2026-11-18)** : étapes dans l'ordre (C0, S0, S1, S2…) ; décisions dans `decisions.md` (raison + alternative écartée) ; rapport court `docs/reports/serveur-<étape>.md` par étape ; un commit par étape, **pas de push** ; spec inchangée (texte proposé dans `docs/reports/spec-proposee.md`) ; ne pas décider ce qui touche au gameplay, aux invariants de `CLAUDE.md` ou à `generator_version` (le noter, continuer) ; tests verts avant l'étape suivante ; ce fichier à jour à chaque commit. Le matin : synthèse de tous les rapports. Reprise automatique prévue à 2 h 15 (tâche de session).

## Étape en cours
S4 — client (rendu extrait du visualiseur, client réseau, horizon, cache disque).

## Fait
- Roadmap serveur écrite (`docs/roadmap-serveur.md`).
- C0 : `write_durable`, journal créé d'un coup. Rapport `serveur-C0.md`.
- S0 : bin `netbench` (codecs, fsync), rapport `serveur-S0.md`, texte proposé `spec-proposee.md` (§5 journal global, §7 réseau).
- S1 : journal global dans `crates/voxel/src/delta.rs` (segments `deltas/wal/`, LSN + CRC-32C, `flush` = fsync groupé, points de reprise `checkpoint_bytes`, région format 2, migration des anciens `.log`), test du processus tué, `netbench wal` (320 k éditions/s). Rapport `serveur-S1.md`.
- S2 : crate `crates/protocol` (`chunk` v2 + `Column` zstd, `messages` postcard, `frame`, `entity` quantifié 1/64 m, `clock`, `varint`) ; colonne v2 = 1 290 o terre / 481 o mer. Rapport `serveur-S2.md`.
- S3 : `crates/server` (config TOML, cache de colonnes partagé, sim 20 Hz, QUIC quinn/rustls-ring, certificat épinglé, flush 50 ms), `crates/client` (connect headless + `Mirror`), `netbot`, test d'intégration `crates/server/tests/edit.rs` (édition vue par 2 clients, refus hors colonnes, client tardif, redémarrage). Arrivée 797 colonnes en 5,3 s (mer, 128 Ko/s). Rapport `serveur-S3.md`.

## Prochaine action
S4 : regarder `crates/viewer` (fly.rs : chargement des colonnes, maillage ; horizon.rs) et décider le découpage : soit une bibliothèque de rendu partagée, soit un mode « réseau » du visualiseur où la source des colonnes est `nascio-client` au lieu de la génération locale. Attention invariant 5 : en mode réseau, aucune génération locale (ni chunks ni horizon). L'horizon réseau demande des tuiles de hauteurs servies par le serveur (nouveau message, `PROTOCOL_VERSION` 2). Commencer petit : mode `--connect addr --cert path` qui affiche les colonnes reçues, horizon désactivé ou en tuiles.

## Décisions prises
- C0 : écriture durable (tmp, fsync, rename, fsync du dossier sous Unix) ; pas de fsync du dossier sous Windows.
- S0 : codec v2 = varint dans l'ordre des indices + zstd-3 par colonne ; crate `zstd` ; fsync groupé toutes les 50 ms ; budget de 128 Ko/s par client.
- S1 : journal global, écriture immédiate + `flush` groupé ; fin invalide du dernier segment retirée, ailleurs refus ; le visualiseur fait un `flush` à chaque édition.
- S2 : postcard pour le contrôle, formats sur mesure pour chunks et entités ; v2 dans `protocol`, v1 reste dans `voxel` ; entités au 1/64 m. Question ouverte (invariant 5) : séparer les types de chunk de la génération pour le client.
- S3 : version de colonne = LSN de région lu avant les chunks ; abonnement avant lecture ; un flux uni par colonne ; ring + certificat épinglé ; crate `nascio-client` ; air au-dessus d'une colonne. Questions : budget 128 vs 256 Ko/s, point d'apparition (gameplay), portée des éditions (gameplay, S7).

## Pièges rencontrés
- PowerShell `Get-Content`/`Set-Content` cassent l'UTF-8 : écrire avec l'outil Write/Edit.
- Les chunks de `chunk_range` sont à 85 % uniformes : mesurer le non uniforme à part.
- Python via Bash : ouvrir en écriture avec `newline='\n'` pour garder LF. Éviter les gros heredocs avec des apostrophes françaises dans une même commande Bash (erreur de quoting) : passer par Write.
- Le test d'intégration du serveur et `cargo test --workspace` dépassent 10 min avec le profil dev au total : lancer en arrière-plan.
- Le serveur lancé à la main sur le monde de référence : `--data out/s3-data` pour ne pas toucher `out/m7/world-42`.
- Un test qui relance son propre exécutable (`current_exe`, `--exact --ignored`) sert de test de processus tué.
