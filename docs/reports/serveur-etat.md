# État du travail — Phase « Serveur et réseau »

À relire après chaque compactage, avec `CLAUDE.md` et `docs/roadmap-serveur.md`.

**Consignes de la nuit (2026-11-18)** : étapes dans l'ordre (C0, S0, S1, S2…) ; décisions dans `decisions.md` (raison + alternative écartée) ; rapport court `docs/reports/serveur-<étape>.md` par étape ; un commit par étape, **pas de push** ; spec inchangée (texte proposé dans `docs/reports/spec-proposee.md`) ; ne pas décider ce qui touche au gameplay, aux invariants de `CLAUDE.md` ou à `generator_version` (le noter, continuer) ; tests verts avant l'étape suivante ; ce fichier à jour à chaque commit. Le matin : synthèse de tous les rapports. Reprise automatique prévue à 2 h 15 (tâche de session).

## Étape en cours
Phase Serveur et réseau validée par l'auteur, S6 compris : mesure sur deux machines, en Wi-Fi, à 250 et 500 bots. Le changement de régime a disparu. Corrections de la mesure faites : tampons UDP, hachages limités, rafales après coupure, `swarm` sans téléportation. Le tout est poussé. Chantiers de l'auteur : **chantier 1 validé et poussé** (crate `nascio-chunk`, protocole v5, rapport `chantier-1.md`, `CLAUDE.md` mis à jour). Chantier 2 (comptes) : plan présenté, en attente de l'accord de l'auteur.

## Fait
- Roadmap serveur écrite (`docs/roadmap-serveur.md`).
- C0 : `write_durable`, journal créé d'un coup. Rapport `serveur-C0.md`.
- S0 : bin `netbench` (codecs, fsync), rapport `serveur-S0.md`, texte proposé pour la spec (§5 journal global, §7 réseau), intégré à `world-spec.md` §5 et §8 le 2026-11-18.
- S1 : journal global dans `crates/voxel/src/delta.rs` (segments `deltas/wal/`, LSN + CRC-32C, `flush` = fsync groupé, points de reprise `checkpoint_bytes`, région format 2, migration des anciens `.log`), test du processus tué, `netbench wal` (320 k éditions/s). Rapport `serveur-S1.md`.
- S2 : crate `crates/protocol` (`chunk` v2 + `Column` zstd, `messages` postcard, `frame`, `entity` quantifié 1/64 m, `clock`, `varint`) ; colonne v2 = 1 290 o terre / 481 o mer. Rapport `serveur-S2.md`.
- S3 : `crates/server` (config TOML, cache de colonnes partagé, sim 20 Hz, QUIC quinn/rustls-ring, certificat épinglé, flush 50 ms), `crates/client` (connect headless + `Mirror`), `netbot`, test d'intégration `crates/server/tests/edit.rs` (édition vue par 2 clients, refus hors colonnes, client tardif, redémarrage). Arrivée 797 colonnes en 5,3 s (mer, 128 Ko/s). Rapport `serveur-S3.md`.
- S4a : `ChunkTint` dans la colonne (protocole v2), `Mirror::padded_chunk`, test `crates/server/tests/mesh.rs` : maillage réseau = maillage local (2 sites, 95 chunks, 17 763 quads). Rapport `serveur-S4a.md`. Question ouverte (invariant 5) : ciel et horizon du client autonome.
- S4b : `crates/viewer/src/remote.rs`, `--connect <addr> [--cert]` : colonnes du serveur maillées depuis le `Mirror`, remaillage à l'arrivée et sur `Blocks`, mode dieu → `SetBlock`, `Move` 4×/s, vue ≤ rayon − 64 m. Vérifié par `--screenshot` (`out/s4b-net.png`, 613 colonnes, 0 manquante). Rapport `serveur-S4b.md`.
- S5 : protocole v3 (datagrammes d'entités numérotés, `Ack`, base par entité), `crates/server/src/replicate.rs` (grille 32 m, priorité, 64/tick + 5 Hz, retraits), `nascio-client::Entities` (interpolation 100 ms sur le tore), pavés des joueurs dans le visualiseur, `netbot --walk`. Test `crates/server/tests/entities.rs` (150 ms, gigue 30 ms, 5 % de pertes : écart 1,4 cm, vitesse 0,501/0,5). Rapport `serveur-S5.md`. Non décidé (gameplay) : autorité du mouvement, collision.
- S6 : bin `swarm` (500 bots dans 200 m, 20 Hz, acquittements, éditions) ; réplication en vecteur trié + pool dédié de 4 fils ; datagrammes par canal vers une tâche réseau par client (place libre publiée, tampon 4 ticks, client sauté si lien plein) ; bug `TooLarge` corrigé ; anneau de colonnes non reparcouru. Tick p99 ≤ 49,4 ms (≤ 42,6 avec swarm en priorité basse), ≤ 34 Ko/s par bot, 110 Mbit/s, ~970 Mo. Changement de régime vers 60-70 s attribué à la cohabitation avec `swarm` (à refaire sur machines séparées). Rapport `serveur-S6.md`.
- S7 : `accounts.rs` (Argon2id, `accounts.toml`), protocole v4 (nom + secret, `Correct`), `security.rs` (seaux de jetons, `LoginGuard`, `abuse.log`), contrôles vitesse/portée, console (`help`, `list`, `kick`, `stats`, `adduser`, `stop`), visualiseur `--name/--secret`. Test `crates/server/tests/security.rs`. Rapport `serveur-S7.md`. Non décidé : valeurs de vitesse/portée, inscription, bannissement.

## Prochaine action
Chantiers décidés par l'auteur (`decisions.md`, décisions du 2026-11-18), dans l'ordre proposé :
1. ~~crate `nascio-chunk` séparée de la génération (définitions des blocs envoyées à la connexion, protocole v5)~~ fait (`chantier-1.md`) ; texte proposé pour `CLAUDE.md` dans le rapport ;
2. ~~comptes : bannissement, inscription fermée, jetons Ed25519 émis par le site (protocole v6)~~ fait, validé et poussé (`chantier-2.md`) ; ajouts après revue : format du jeton et vecteurs fixes dans la spec §7.2, refus de `server_id = "dev"` hors local (`bind` local par défaut), console `>_` et `help <commande>` ;
2bis. **Gisements de surface et nucléaires** (prompt de l'auteur, intercalé avant le chantier 3) : plan validé le 2026-11-18, à coder. Le plan, les réponses et les sous-étapes G1 à G3 sont dans `docs/reports/gisements-surface.md`. `bdd5c44` est poussé (format du jeton, `server_id`, console) ;
3. sites d'apparition et point par compte (calculés au démarrage du serveur, sans toucher `generator_version`) ;
4. horizon en tuiles et climat réduit envoyés par le serveur ;
5. mouvement autoritaire, avec prédiction et réconciliation.

À faire au prochain changement de format du monde (décision de l'auteur, 2026-11-18) : le feuillage devient une propriété de `blocks.toml` (aujourd'hui déduit du nom, `nascio_chunk::block::FOLIAGE`).

Questions encore ouvertes pour l'auteur : la définition exacte des classes de terrain des sites ; la physique du joueur, et si le vol libre reste un droit d'administrateur .

Ancienne note de la nuit (la synthèse a été faite) : Le matin, il demandera une synthèse de tous les rapports (`serveur-C0.md` … `serveur-S7.md`, `spec-proposee.md`) : ce qui est fait, mesures clés, décisions, et la liste regroupée des questions ouvertes (hébergement, budget, rayons, gameplay : point d'apparition, mouvement/collision, vitesse/portée ; invariant 5 : ciel/horizon du client, types de chunk ; inscription, bannissement ; seconde machine pour S6). Rien n’est poussé : 10 commits locaux de la phase (C0 à S7, S4 en deux).

## Décisions prises
- Chantier 2.2-2.3 : jeton Ed25519 (`nascio_server::token`, outil `site`) 5 min, usage unique, refusé s'il est émis avant démarrage + 30 s ; protocole v6 `ClientHello.auth` = `Secret`/`Token` ; `site_public_key`, `server_id`, `secret_login` (`admin` par défaut) ; visualiseur `--token`, `swarm --site-key`.
- Chantier 2.1 : bannissement par compte dans `accounts.toml` (vérifié après le secret, expulse, `abuse.log`) ; console `ban`/`unban`/`bans`/`admin` ; `open_registration` faux par défaut, `--open-registration` pour le développement.
- Chantier 1 : crate `nascio-chunk` (blocs, chunks, teinte, maillage ; sans zstd ni bruit), réexportée par `world`/`voxel` ; codec v2 et `Column` restent dans `protocol` ; v5 = définitions des blocs dans `ServerHello` ; garde-fou `crates/client/tests/no_generation.rs`.
- C0 : écriture durable (tmp, fsync, rename, fsync du dossier sous Unix) ; pas de fsync du dossier sous Windows.
- S0 : codec v2 = varint dans l'ordre des indices + zstd-3 par colonne ; crate `zstd` ; fsync groupé toutes les 50 ms ; budget de 128 Ko/s par client.
- S1 : journal global, écriture immédiate + `flush` groupé ; fin invalide du dernier segment retirée, ailleurs refus ; le visualiseur fait un `flush` à chaque édition.
- S2 : postcard pour le contrôle, formats sur mesure pour chunks et entités ; v2 dans `protocol`, v1 reste dans `voxel` ; entités au 1/64 m. Question ouverte (invariant 5) : séparer les types de chunk de la génération pour le client.
- S3 : version de colonne = LSN de région lu avant les chunks ; abonnement avant lecture ; un flux uni par colonne ; ring + certificat épinglé ; crate `nascio-client` ; air au-dessus d'une colonne. Questions : budget 128 vs 256 Ko/s, point d'apparition (gameplay), portée des éditions (gameplay, S7).
- S7 : Argon2id + `accounts.toml` ; v4 nom + secret ; limites sur le fil de lecture ; `Correct` ; `abuse.log` limité par type ; vérifier le secret avant de dire « déjà connecté ».
- S6 : vecteur trié + `select_nth` ; pool de réplication dédié (4) ; simulation découplée de QUIC (canal, place libre publiée, tampon 4 ticks, client sauté) ; anneau de colonnes non reparcouru.
- S5 : `seq` par datagramme + base par entité, `Ack` sur le contrôle ; priorité par accumulateur, 64/tick + 5 Hz, rayon 256 m, budget partagé (entités d'abord) ; affichage à 100 ms ; mouvement décidé par le client (autorité et collision = gameplay, non décidés).
- S4b : fil réseau + `Mirror` sous mutex ; édition affichée après confirmation du serveur (prédiction = question de ressenti) ; vue ≤ rayon − 64 m.
- S4a : teinte et profondeur d'eau envoyées avec la colonne (f32 exacts) ; sous la plage, répéter le bloc le plus bas ; S4 découpé en S4a/S4b ; source du ciel/horizon du client non décidée (invariant 5).

## Pièges rencontrés
- Bash + Python en ligne : les barres obliques inverses (`\\`, `\x1b`, `\r`, `\n`) sont interprétées avant d'arriver au fichier (continuations de chaînes Rust perdues, caractères de contrôle bruts) : écrire le script avec l'outil Write et utiliser `chr(92)`.
` sont interprétées avant d'arriver au fichier (continuations de chaînes Rust perdues, caractères de contrôle bruts) : écrire le script avec l'outil Write et utiliser `chr(92)`.
- Un compteur de violations commun à tous les types empêchait de journaliser certains types (seuil 1 / 100) : compter par type.
- `RefCell` dans une tâche tokio rend le futur non `Send` : `Mutex`.
- Windows : `cargo build` échoue (« Accès refusé ») si `nascio-server.exe` tourne : le tuer avant de compiler.
- Une valeur publiée seulement après un envoi devient périmée quand on cesse d'envoyer (client sauté pour toujours) : la rafraîchir aussi sur minuteur.
- Mesures de charge : `swarm` et le serveur sur la même machine 6 cœurs se gênent (changement de régime vers 60-70 s).
- Un script Python d'édition qui échoue à mi-chemin laisse un fichier à moitié modifié : relire avant de recompiler.
- clap : `value_delimiter = ','` avec `num_args = 3` refuse « 1,2,3 » ; garder seulement le délimiteur et vérifier la longueur.
- Un client tué sans `close` reste compté par le serveur jusqu'au délai d'inactivité QUIC.
- `use bevy::prelude::*` avec `anyhow::Context` : `.context`/`.with_context` ambigus (E0034, `ContextExt` de Bevy) ; importer seulement les noms utiles du prélude.
- `--screenshot` attend `missing == 0` : en mode réseau, les colonnes du bord sans voisines bloquaient jusqu'au délai maximal.
- PowerShell `Get-Content`/`Set-Content` cassent l'UTF-8 : écrire avec l'outil Write/Edit.
- Les chunks de `chunk_range` sont à 85 % uniformes : mesurer le non uniforme à part.
- Python via Bash : ouvrir en écriture avec `newline='\n'` pour garder LF. Éviter les gros heredocs avec des apostrophes françaises dans une même commande Bash (erreur de quoting) : passer par Write.
- Le test d'intégration du serveur et `cargo test --workspace` dépassent 10 min avec le profil dev au total : lancer en arrière-plan.
- Le serveur lancé à la main sur le monde de référence : `--data out/s3-data` pour ne pas toucher `out/m7/world-42`.
- Un test qui relance son propre exécutable (`current_exe`, `--exact --ignored`) sert de test de processus tué.
