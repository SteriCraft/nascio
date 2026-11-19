# Décisions prises sans consigne de l'auteur depuis le 2026-11-18, 00 h 00 (Paris)

Ce document couvre les commits `f66835b` (00 h 15) à `029d4f9` (12 h 24), plus la sous-étape G1 des gisements, codée mais pas encore commitée. Les décisions dictées par l'auteur n'y figurent pas : questions tranchées, demandes explicites, lignes « Décision de l'auteur » de `decisions.md`. Les rapports d'étape cités par les commits restent lisibles dans l'historique git.

Réversibilité :
- **facile** : du code seul, sans effet sur les données ni sur les autres programmes ;
- **version** : il faut incrémenter `PROTOCOL_VERSION`, et les clients doivent suivre ;
- **migration** : un format sur disque change, et il faut convertir les fichiers existants ;
- **générateur** : il faut incrémenter `generator_version`, et le monde de référence change.

> **Clos (2026-11-19)** : les points ci-dessous ont été tranchés depuis (gisements G1 à G3 validés et version 20 figée ; `Move` retiré au chantier 5 ; spec §5 et §8 intégrées le 2026-11-18). Le seul reste, l'adresse d'écoute par défaut, est une note de déploiement dans `docs/limites.md`.

## 1. À faire valider

Ces décisions touchent au protocole, aux formats sur disque, à la sécurité, ou au contenu du futur monde de référence. Tu ne les as pas validées une par une.

| Décision | Étape, commit | Raison | Alternative écartée | Réversible |
|---|---|---|---|---|
| Au-delà de la limite de messages, les `Move` et `Ack` sont jetés **sans compter pour l'expulsion** (type `burst` dans le journal). Les autres messages restent comptés. | Après S6, `8a59d60` | Après une coupure Wi-Fi, un joueur honnête frôlait l'expulsion : 400 refus sur 500. | Relever la rafale ; ne plus rien compter. | Facile. **Sécurité** : un client peut inonder de `Move` sans être expulsé, mais l'excès est jeté. Ta validation de S6 couvre peut-être ce point : à confirmer. |
| `bind` par défaut passé de `0.0.0.0` à `127.0.0.1:7777`. Un `server_id` vide est refusé, comme `"dev"`, hors adresse locale. | Après la revue du chantier 2, `bdd5c44` | Un serveur lancé sans option reste utilisable en local. Un serveur public doit tout donner explicitement. | Garder `0.0.0.0` et refuser de démarrer sans `--server-id`. | Facile. **Déploiement** : le serveur Linux doit passer `--bind 0.0.0.0:7777 --server-id <nom>`. |
| Nouveau bloc `chalk` (craie), ajouté **en fin** de `BUILTIN` (31ᵉ indice) : couleur [236, 232, 220], densité 2 000. | G1, non commité | Ajouté en fin de table, il garde les indices des blocs existants. | L'insérer près du calcaire, ce qui décale les indices. | Générateur. **Format** : la table des blocs du monde et celle envoyée aux clients (v5) changent. |
| Part de la craie dans les bancs sédimentaires. Marin : calcaire 0,30, craie 0,15, argile 0,45, grès 0,10, au lieu de calcaire 0,45, argile 0,45, grès 0,10. Plate-forme humide : grès 0,35, argile 0,30, calcaire 0,20, craie 0,15. Ailleurs, rien ne change. Un registre sans craie retombe sur le calcaire. | G1, non commité | La craie est un carbonate pélagique de plate-forme ; les anciens mondes restent lisibles. | Ajouter la craie en plus du calcaire sans le réduire, ce qui donne trop de carbonates ; la mettre aussi dans les bassins. | Générateur, et coûteux une fois `out/gisements/world-42` devenu la référence. |
| Sémantique de `host_rock` : la condition **s'ajoute** au filtre de la forme. Le corps ne remplace qu'une roche de la bonne classe **et** de la liste. Les noms sont vérifiés au chargement : un bloc inconnu ou un nom vide est une erreur. Le champ est omis du catalogue écrit quand il est vide. | G1, non commité | Le corps garde la forme qu'il avait. Un catalogue sans le champ s'écrit comme avant. | `host_rock` qui remplace le filtre de la forme : un filon pourrait alors entrer dans la terre ou le sable. | Migration, puisque c'est le format du catalogue `deposits.toml` copié dans chaque monde. |
| Conception de `above` (**pas encore codé**). Chaque corps du type parent reçoit un corps enfant avec la probabilité `min(1, abundance/100 × poids du contexte)`, centré sur le x et le z du parent. RNG `deposits/<nom>/above/<i>`, placement après le parent. Le parent doit être un type antérieur, sans `above` lui-même et pas un piège. L'enfant a une abondance ≤ 100 et n'est ni un placer ni un piège. | G2, plan dans `gisements-surface.md` | Le tirage est déterministe et l'ordre est fixe. Le sens de `abundance` change (un pourcentage) pour rester lisible. | Placer l'enfant dans les tuiles près des parents, ce qui demande un rayon de plus et ne garantit pas l'aplomb. | Migration et générateur : format du catalogue et étiquettes des RNG. |
| Paramètres des huit nouveaux types : contextes, formes, profondeurs, tailles, teneurs, abondances. Couleurs et densités des sept nouveaux blocs. | Plan, `029d4f9` | Viser environ 0,1 % du volume, avec du cuivre et du fer trouvables à pied. | Voir la table du plan. | Générateur. Tu as répondu aux questions du plan sans commenter cette table : à confirmer avant G3. |

## 2. Détails techniques

| Décision | Étape, commit | Raison | Alternative écartée | Réversible |
|---|---|---|---|---|
| Tampons UDP de 4 Mio demandés par `socket2` (`udp_buffer_bytes`), avec un avertissement si le noyau en accorde moins. | Après S6, `8a59d60` | Des pertes UDP pendant les rafales de connexions dépassaient le délai du client. | Tampon par défaut ; allonger le délai. | Facile |
| Au plus `account_hash_concurrency` hachages Argon2id à la fois (la moitié des cœurs). | Après S6, `8a59d60` | Une rafale de connexions ne prend plus les cœurs du tick. | Aucune limite ; un seul fil. | Facile |
| `swarm` fait marcher ses bots jusqu'à leur place au lieu de les téléporter. | Après S6, `8a59d60` | La téléportation déclenchait le contrôle de vitesse. | Désactiver le contrôle pour les bots. | Facile |
| Décodage base64url strict : refus du remplissage et des bits de fin non nuls. | Chantier 2.2 | Un seul encodage par jeton, sans malléabilité. | Décodeur tolérant. | Facile, mais le site doit encoder canoniquement, ce que tout encodeur standard fait. |
| Vecteurs de test tirés de la graine `00…1f`. Le test reconstruit la charge octet par octet, sans postcard. | Après la revue, `bdd5c44` | Le test vérifie la description de la spec, pas seulement le code. | Vecteurs produits par le code seul. | Facile |
| Console : séquences ANSI seulement sous `WT_SESSION` ou `TERM_PROGRAM` sur Windows, aucune hors terminal. Curseur souligné clignotant (`ESC[3 q`). Une table unique pour `help` et `help <commande>`. | Après la revue, `bdd5c44` | Pas de caractères parasites dans l'ancienne console ni dans les journaux. | Une crate de terminal (crossterm, rustyline). | Facile |
| `body_at`, `ore` et `ore_kind` reçoivent la roche remplacée (`rock`). | G1 | C'est le seul endroit où le filtre `host_rock` peut se faire. | Un second passage après la génération. | Facile |
| `host_rock` documenté dans l'en-tête de `config/deposits.toml`. Comme le catalogue est haché sur son texte, le hachage du monde change. | G1 | Le catalogue se lit seul. | Documenter seulement la spec. | Générateur, sans enjeu : v20 change de toute façon. |
| Valeurs dorées v20 (`e221acae…`, chunk 16 `f6c62369954e0e57`). | G1 | Elles suivent le changement de générateur. | — | Facile |
| G2 : extraire la mise en forme d'un corps (`shape_body`) en gardant l'ordre des tirages, pour que les mondes sans `above` gardent le même hachage. | G2, prévu | Un enfant `above` réutilise la même mise en forme. | Dupliquer le code. | Facile |
| Mesure à 2 km : une option de l'outil `geology`, sans toucher à la génération. | G3, prévu | C'est une vérification, pas une donnée du monde. | Une couche de plus. | Facile |
| Le monde d'essai est généré dans `out/gisements/` ; `out/m7/world-42` n'est pas touché. | Plan, `029d4f9` | Les deltas de `out/s3-data` dépendent de l'ancien monde. Tu as confirmé ce choix. | — | Facile |

## 3. Déjà validées par toi

Ce sont mes décisions de la phase serveur (C0 à S7), que tu as validées avec la phase, et celles des chantiers 1 et 2, que tu as validés. Plusieurs restent coûteuses à changer, ce qui est noté dans la dernière colonne.

| Décision | Étape, commit | Raison | Alternative écartée | Réversible |
|---|---|---|---|---|
| Écriture durable : fichier temporaire, `fsync`, renommage, `fsync` du dossier sous Unix. Journal créé d'un coup avec son en-tête. | C0, `f66835b` | Une coupure de courant pouvait perdre une région. | `fsync` par édition ; `fsync` du dossier sous Windows. | Facile |
| Codec de chunks v2 : plages varint dans l'ordre des indices, puis zstd-3 par colonne. | S0, `ff4d6f7` | 1 277 o par colonne au lieu de 3 866. | Ordre y d'abord ; bits compactés ; LZ4. | Version |
| Crate `zstd`, liaison C. | S0, `ff4d6f7` | C'est l'implémentation de référence. | zstd en Rust pur ; LZ4. | Facile |
| `fsync` groupé toutes les 50 ms. | S0, `ff4d6f7` | `fsync` par édition : 2 110 éditions/s au plus. Groupé : 156 586/s, pour une perte d'un tick au plus. | `fsync` par édition ; aucun `fsync`. | Facile |
| Budget de 128 Ko/s par client, dans l'ordre entités, éditions, chunks. | S0, `ff4d6f7` | Une bataille de 500 joueurs demande environ 41 Ko/s. | Toutes les entités à 20 Hz. | Facile ; la valeur est confirmée par toi. |
| Journal global des deltas : `deltas/wal/`, LSN, CRC-32C, points de reprise, régions au format 2. | S1, `ca0cb1e` | Un seul `fsync` par intervalle et un ordre global. | Un journal par région ; SQLite ou sled. | **Migration**. **Contredit `world-spec.md` §5** (voir plus bas). |
| Fin invalide du dernier segment retirée ; une erreur ailleurs fait échouer l'ouverture. | S1, `ca0cb1e` | Une coupure ne tronque que la fin. | Tronquer au premier enregistrement invalide. | Facile |
| Le visualiseur fait un `flush` après chaque édition. | S1, `ca0cb1e` | Les éditions sont rares. | Synchroniser à la fermeture. | Facile |
| Crate `nascio-protocol` sans réseau, postcard pour le contrôle, formats sur mesure pour chunks et entités. | S2, `b317045` | Données pures, testables. | bincode, JSON, Protocol Buffers. | Version |
| v2 dans `protocol`, v1 reste dans `voxel`. Entités au 1/64 m, lacet sur 16 bits, tangage sur 8 bits. | S2, `b317045` | Erreur < 8 mm, de 5 à 7 octets par déplacement. | `f32` bruts ; 1/16 m. | Version |
| Un seul processus, simulation sur son fil à 20 Hz, réseau sur tokio. | S3, `3b586a5` | Durée de tick prévisible. | Simulation dans tokio. | Facile |
| Version de colonne = LSN de la région, lu avant les chunks. Abonnement avant lecture. | S3, `3b586a5` | Aucune édition perdue entre lecture et abonnement. Valable pour un cache client. | Compteur en mémoire ; un seul flux. | Version |
| Un flux unidirectionnel par colonne, 16 en vol par client. | S3, `3b586a5` | Pas de blocage en tête de file. | Un flux ; des datagrammes. | Version |
| TLS rustls et ring, certificat auto-signé épinglé. | S3, `3b586a5` | Pas de CMake ni de NASM sous Windows. | aws-lc-rs ; pas de vérification. | Facile, à revoir pour l'hébergement. **Sécurité.** |
| Crate `nascio-client` ; seulement de l'air au-dessus d'une colonne. | S3, `3b586a5` | Une seule implémentation côté client. | Réseau dans `protocol`. | Facile |
| Teinte et profondeur d'eau en `f32` exacts dans la colonne. Sous la plage, le bloc le plus bas est répété. | S4a, `8de4ede` | Maillage réseau identique bit à bit au maillage local. | Couleurs sur 8 bits. | Version |
| Fil réseau et `Mirror` sous mutex. Édition affichée après confirmation du serveur. Vue ≤ rayon − 64 m. | S4b, `2327698` | Bevy n'attend jamais le réseau. | tokio dans Bevy ; prédiction. | Facile |
| Datagrammes numérotés, base par entité, `Ack` sur le contrôle. | S5, `0899dda` | La priorité rend les instantanés partiels. | Base unique par instantané. | Version |
| Priorité par accumulateur, 64 entités par tick plus un plancher à 5 Hz, rayon de 256 m. | S5, `0899dda` | 5 Hz garantis à tous. | Tri par distance. | Facile |
| Affichage 100 ms derrière le serveur, interpolation sur le tore, extrapolation ≤ 250 ms. | S5, `0899dda` | Absorbe une perte et la gigue. | 50 ms. | Facile |
| Réplication en vecteur trié, pool dédié de 4 fils. | S6, `e709104` | De 80 à 15-19 ms par tick. | `BTreeMap` ; pool global. | Facile |
| Simulation découplée de QUIC : canal, place libre publiée, tampon de 4 ticks, client sauté si son lien est plein. | S6, `e709104` | Le tick n'attend plus les verrous. | `send_datagram` depuis la simulation. | Facile |
| Anneau de colonnes non reparcouru. | S6, `e709104` | Serveur de 3,4 à 2,8 cœurs. | File d'événements. | Facile |
| Argon2id (paramètres par compte), sel de ring, `accounts.toml` réécrit de façon durable, protocole v4. | S7, `d6698a2` | C'est le standard ; on peut durcir sans casser les comptes. | bcrypt, PBKDF2 ; une base de données. | **Migration**, **sécurité** |
| Seaux de jetons, expulsion après 500 refus, `LoginGuard` à 5 échecs par minute, `Correct`, `abuse.log` limité par type. | S7, `d6698a2` | Un abus ne coûte rien au tick. | Contrôles dans la simulation. | Facile |
| Crate `nascio-chunk`. Codec v2 et `Column` restent dans `protocol`. | Chantier 1, `2d18026`, `6eb2948` | L'invariant 5 est garanti par la compilation. | Types dans `voxel`. | Facile |
| v5 : définitions des blocs dans `ServerHello`, feuillage déduit du nom. | Chantier 1, `6eb2948` | Le client ne lit plus de fichier de blocs. | `foliage` dans `blocks.toml` (prévu au prochain changement de format). | Version |
| Bannissement dans `accounts.toml`, vérifié après le secret, gardé après expiration. | Chantier 2.1, `c7cb4f8` | Un inconnu n'apprend rien ; l'historique est conservé. | `bans.toml` ; suppression à l'expiration. | Migration |
| Jeton `postcard(Claims) ‖ sig`, ≤ 512 caractères, `ring`, base64 écrit à la main, outil `site`. | Chantier 2.2, `0296869` | Aucune nouvelle dépendance. | JWT, PASETO. | **Format partagé avec le site** |
| Borne stricte : un jeton émis avant démarrage + 30 s est refusé. Jeton consommé même si la connexion échoue ensuite. | Chantier 2.2 | Pas de rejeu après un redémarrage. | Démarrage − 30 s ; nonces sur disque. | Facile. Fait partie des quatre décisions validées. |
| Protocole v6 `Auth::Secret` ou `Auth::Token`. Un jeton crée le compte sans secret. `--open-registration` ouvre aussi les secrets à tous. | Chantier 2.3, `297e8ce` | Les joueurs passent par le site, les secrets sont réservés aux admins. | Champ `token` à côté du secret. | Version, **sécurité** |

## Manques dans `decisions.md`
- **G1** : la ligne du 2026-11-18 annonce `host_rock` et la craie. Il y manque la place du bloc en fin de table, les parts de craie dans les tables et la sémantique de `host_rock`. Je comptais l'ajouter au commit de G1.
- **G2** : la conception de `above` n'est pas encore dans le journal, puisqu'elle n'est pas codée.
- **Paramètres des huit types** : ils ne sont que dans `gisements-surface.md`.
- **Détails mineurs absents** : décodage base64 strict, `swarm` qui marche jusqu'à sa place.

## Contradictions avec la spec
- **`world-spec.md` §5 (deltas)** décrit un journal par région (`r.<rx>.<rz>.log`, compaction après `compact_after`). Depuis S1 (`ca0cb1e`), le code utilise un journal global (`deltas/wal/`) et des régions au format 2, et migre les anciens journaux. Le texte de remplacement est proposé dans `spec-proposee.md` §5 mais pas encore adopté. **La spec et le code divergent.**
- **Réseau** : la spec ne décrit ni le protocole v6 ni le format du jeton, seulement proposés dans `spec-proposee.md` §7. Ce n'est pas une contradiction mais une lacune, à combler en adoptant le texte proposé.
- Aucune autre contradiction trouvée : §4.6 est mis à jour pour `host_rock` et la craie dans G1.
