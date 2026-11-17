# Texte proposé pour la spec (phase Serveur et réseau)

Ce fichier ne modifie pas `docs/world-spec.md` : il propose un texte, à valider par l'auteur avant d'être reporté dans la spec. Chaque section remplace ou complète la section de la spec indiquée.

## §5 — Remplace les points « Fichiers » et « Journal et chargement »

- **Fichiers** : `world-<seed>/deltas/r.<rx>.<rz>.bin`, une région de 25 × 25 colonnes de chunks (800 m), toutes hauteurs. En-tête : `NSCDELTA`, format (2), `generator_version`, empreinte de génération, **numéro de séquence (LSN) du dernier enregistrement du journal appliqué à la région**, puis les noms des blocs utilisés. Un fichier de format 1 (sans LSN) se lit avec un LSN nul. Écriture durable : fichier temporaire, `fsync`, renommage, `fsync` du dossier (Unix).
- **Journal** : un journal **global** en ajout seul, découpé en segments `world-<seed>/deltas/wal/w.<lsn>.log` (LSN du premier enregistrement, 16 chiffres hexadécimaux). En-tête d'un segment : `NSCWAL`, format, `generator_version`, empreinte. Enregistrement : longueur (`u32`), LSN (`u64`), type (`u8`), contenu, CRC-32C (sur LSN, type et contenu). Type 1 : édition de bloc (`x, y, z`, nom du bloc). Les autres types sont réservés aux états de jeu à venir (inventaires…), qui devront passer par le même journal pour qu'une action reste atomique.
- **Validation groupée** : une édition est écrite aussitôt dans le segment courant (le système l'a : elle survit à l'arrêt brutal du programme), puis appliquée en mémoire ; `flush` fait un seul `fsync` pour toutes les éditions depuis le précédent (elles survivent alors à une coupure de courant). Le serveur l'appelle à chaque intervalle `fsync_interval_ms` (50 ms, un tick). Après une coupure de courant, le monde revient en arrière d'au plus un intervalle, de façon cohérente (le journal est ordonné).
- **Points de reprise** : quand le journal dépasse `checkpoint_bytes` (16 Mio), un nouveau segment est ouvert, chaque région modifiée depuis le dernier point de reprise est réécrite (écriture durable, avec son LSN), puis les anciens segments sont supprimés. Une région déchargée de la mémoire alors qu'elle est modifiée est d'abord réécrite.
- **Chargement** : l'ouverture lit les en-têtes des régions et parcourt le journal une fois (index des enregistrements par région). Une région est chargée à sa première lecture : son fichier, puis ses enregistrements de LSN supérieur au sien. Un enregistrement incomplet ou de CRC faux à la fin du dernier segment est une écriture interrompue : il est retiré, avec tout ce qui le suit. Une erreur ailleurs est une corruption : l'ouverture échoue. Les anciens journaux par région (`r.<rx>.<rz>.log`) sont rejoués une fois, puis la région est réécrite et le journal supprimé.

## §7 — Réseau (nouvelle section)

### 7.1 Architecture
- Un seul processus serveur, autoritaire, sans découpage du monde. Simulation à tick fixe de 20 Hz sur son propre fil ; réseau asynchrone sur d'autres fils. Les entrées des clients sont traitées par identifiant de joueur croissant.
- Transport QUIC (TLS 1.3) : un flux bidirectionnel de contrôle, des flux unidirectionnels pour les chunks, des datagrammes non fiables pour les états des entités.

### 7.2 Poignée de main
Le client envoie la version du protocole ; le serveur répond avec `generator_version`, l'empreinte de génération, la table des blocs (nom → identifiant de la session) et son heure UTC. Le client mesure le décalage de son horloge (aller-retour, à la manière de NTP) et dérive le temps du jeu de l'UTC corrigée : aucune horloge interne.

### 7.3 Chunks
- Codec v2 : un octet de version, la palette (nombre puis identifiants, varints), puis des plages (indice de palette, longueur − 1), varints, dans l'ordre des indices des blocs (`x`, puis `z`, puis `y`). Un chunk uniforme tient en 4 octets.
- Les chunks voyagent par lots compressés en zstd (niveau 3) ; l'unité de cache du serveur est la colonne de chunks encodée et compressée, faite une fois et partagée entre les clients, invalidée par une édition.
- Le serveur choisit les colonnes envoyées (rayon `chunk_radius_m`, 512 m par défaut ; colonnes les plus proches d'abord). Au-delà, l'horizon est servi en tuiles de hauteurs précalculées. Le client ne génère jamais de terrain.
- Les éditions sont diffusées par chunk à la fin de chaque tick (indice local, bloc) ; au-delà de `resend_after` éditions dans un chunk pendant un tick, le chunk entier est renvoyé.
- Le client peut garder les chunks reçus dans un cache disque, indexé par l'empreinte de génération, la position et la version du chunk (compteur d'éditions).

### 7.4 Entités et débit
- Grille spatiale de 32 m ; pour chaque client, un accumulateur de priorité (distance, ancienneté du dernier envoi) ; les 64 entités les plus prioritaires à 20 Hz, les autres à 5 Hz au moins.
- Positions quantifiées, envoyées en différence par rapport au dernier état confirmé par le client.
- Budget par client `client_budget_bytes_per_s` (128 Ko/s) : entités, puis éditions, puis chunks.
