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
Le client envoie la version du protocole, son nom de compte et une preuve d'identité, dans le canal chiffré (certificat du serveur épinglé) :
- **un jeton du site**, cas normal des joueurs. Le site, qui gère l'inscription et l'invitation, signe en Ed25519 `base64url(postcard(Claims) ‖ signature)`, où `Claims` = {version, nom, serveur visé (`server_id`), émission et expiration en ms UTC, nonce de 16 octets}. Le serveur ne connaît que la clé publique (`site_public_key`). Il vérifie, dans l'ordre :
  1. la signature ;
  2. la version, l'audience et le nom (celui du message) ;
  3. une validité d'au plus 5 min ;
  4. l'expiration et l'émission dans le futur, avec ±30 s de tolérance d'horloge ;
  5. une émission postérieure au démarrage du serveur + 30 s, car les nonces sont gardés en mémoire et un redémarrage ne doit pas ouvrir de fenêtre de rejeu ;
  6. un nonce jamais vu (usage unique).

  Un jeton valide crée le compte s'il n'existe pas ;
- **un secret**, réservé par défaut aux comptes administrateurs (`secret_login` = `admin`, `all` ou `none`). Le serveur le vérifie avec Argon2id et un sel par compte.

Ensuite, le serveur refuse un compte banni (bannissement par compte, temporaire ou permanent, avec sa raison, signalé seulement après une preuve valide) et une deuxième session. Il bloque une adresse après trop d'échecs. Enfin, il répond avec `generator_version`, l'empreinte de génération, la table des blocs (leurs définitions) et son heure UTC. Le client mesure le décalage de son horloge (aller-retour, à la manière de NTP) et dérive le temps du jeu de l'UTC corrigée : aucune horloge interne.

#### Format du jeton, octet par octet
Un jeton est la chaîne `base64url(charge ‖ signature)` : alphabet URL de la RFC 4648 §5 (`A–Z a–z 0–9 - _`), **sans** remplissage `=`, au plus 512 caractères. Le serveur refuse une forme non canonique, c'est-à-dire des bits de fin non nuls.

La **charge** enchaîne les champs suivants, dans cet ordre et sans séparateur (c'est l'encodage postcard de `Claims`) :

| Champ | Encodage |
|---|---|
| `version` | 1 octet, vaut `0x01` |
| `name` | longueur en varint, puis les octets UTF-8 (3 à 16 caractères parmi `A–Z a–z 0–9 _`) |
| `server` | longueur en varint, puis les octets UTF-8 (le `server_id` visé) |
| `issued_unix_ms` | entier signé de 64 bits en zigzag, puis en varint |
| `expires_unix_ms` | même encodage ; au plus `issued_unix_ms` + 300 000 |
| `nonce` | 16 octets bruts, aléatoires (pas de longueur) |

Définitions des encodages :
- **varint** (LEB128 non signé) : 7 bits par octet, poids faible d'abord ; le bit `0x80` signale qu'un octet suit.
- **zigzag** : `(v << 1) ^ (v >> 63)`, avec un décalage arithmétique, interprété comme un entier non signé de 64 bits.

La **signature** fait 64 octets. C'est une signature Ed25519 (RFC 8032, sans pré-hachage ni contexte) de la charge, telle quelle. Le serveur la reconnaît comme les 64 derniers octets ; tout le reste est la charge, qui doit se lire exactement, sans octet de trop.

**Vecteurs de test** (vérifiés par `crates/server/src/token.rs::fixed_vectors`). Ed25519 est déterministe : une implémentation correcte doit retrouver exactement ces valeurs.

Entrées :
- graine de la clé privée (32 octets) : `000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f` ;
- `name = "alice"`, `server = "eu-1"`, `issued_unix_ms = 1 800 000 000 000`, `expires_unix_ms = 1 800 000 300 000` ;
- `nonce = a0 a1 a2 … af`.

Clé publique attendue :
- en hexadécimal : `03a107bff3ce10be1d70dd18e74bc09967e4d6309ba50d5f1ddc8664125531b8` ;
- en base64url, la forme de `site_public_key` : `A6EHv_POEL4dcN0Y50vAmWfk1jCbpQ1fHdyGZBJVMbg`.

Charge attendue (40 octets) :
```
01                                  version
05 616c696365                       "alice"
04 65752d31                         "eu-1"
80c0e285e368                        issued  (zigzag 3 600 000 000 000)
c08f8786e368                        expires (zigzag 3 600 000 600 000)
a0a1a2a3a4a5a6a7a8a9aaabacadaeaf    nonce
```

Signature attendue :
```
dcc27d3b4a7db6312a01d13fef72eb799d6d31bd823e03e11cdbdaa09de5ed04
7dd3a820d744721b2871441d61af9c7f0919e3f26684d7fc9ba5f53eab981808
```

Jeton attendu (139 caractères) :
```
AQVhbGljZQRldS0xgMDiheNowI-HhuNooKGio6SlpqeoqaqrrK2ur9zCfTtKfbYxKgHRP-9y63mdbTG9gj4D4Rzb2qCd5e0EfdOoINdEchsocUQdYa-cfwkZ4_JmhNf8m6X1PquYGAg
```

### 7.3 Chunks
- Codec v2 : un octet de version, la palette (nombre puis identifiants, varints), puis des plages (indice de palette, longueur − 1), varints, dans l'ordre des indices des blocs (`x`, puis `z`, puis `y`) ; pas de plages quand la palette n'a qu'une entrée : un chunk uniforme tient en 3 à 5 octets.
- Une colonne transporte aussi ses données de rendu, calculées par le serveur à partir du climat : bloc teinté, couleurs des quatre coins (`f32`), profondeur de l'eau de chaque colonne locale.
- Les chunks voyagent par lots compressés en zstd (niveau 3) ; l'unité de cache du serveur est la colonne de chunks encodée et compressée, faite une fois et partagée entre les clients, invalidée par une édition.
- Le serveur choisit les colonnes envoyées (rayon `chunk_radius_m`, 512 m par défaut ; colonnes les plus proches d'abord). Au-delà, l'horizon est servi en tuiles de hauteurs précalculées. Le client ne génère jamais de terrain.
- Les éditions sont diffusées par chunk à la fin de chaque tick (indice local, bloc) ; au-delà de `resend_after` éditions dans un chunk pendant un tick, le chunk entier est renvoyé.
- Une colonne porte une version : le LSN de sa région (§5) lu avant ses chunks. Un lot d'éditions porte le LSN de sa dernière édition ; le client l'applique à une colonne de version inférieure et le garde tant que la colonne n'est pas arrivée (colonnes et contrôle voyagent sur des flux différents). Au-dessus des chunks d'une colonne, il n'y a que de l'air.
- Le client peut garder les colonnes reçues dans un cache disque, indexé par l'empreinte de génération, la position et la version.

### 7.4 Entités et débit
- Grille spatiale de 32 m ; pour chaque client, un accumulateur de priorité (distance, ancienneté du dernier envoi) ; les 64 entités les plus prioritaires à 20 Hz, les autres à 5 Hz au moins.
- Positions quantifiées (1/64 m ; lacet sur 16 bits, tangage sur 8 bits), envoyées en différence par rapport au dernier état confirmé par le client. Chaque datagramme porte un numéro (`seq`) et le tick du serveur ; le client acquitte les numéros reçus (`Ack`, flux de contrôle). Une entité en différence désigne le datagramme qui lui sert de base (au plus 64 datagrammes plus tôt) ; sinon elle est envoyée en entier. Une entité sortie du rayon (`entity_radius_m`, 256 m) est envoyée comme retirée jusqu'à l'acquittement du retrait.
- Le client affiche les entités 100 ms derrière le serveur, par interpolation sur le tore, avec une extrapolation de 250 ms au plus.
- Budget par client `client_budget_bytes_per_s` (128 Ko/s) : entités, puis éditions, puis chunks.

### 7.5 Limites et abus
- Chaque client a des seaux de jetons pour ses messages de contrôle et ses éditions ; au-delà, le message est ignoré ou l'édition refusée, et après trop de refus le client est expulsé.
- Un déplacement plus rapide que la vitesse maximale est refusé et le joueur est remis à sa dernière position acceptée (`Correct`) ; une édition hors de portée est refusée. Les valeurs (vitesse, portée) relèvent du gameplay.
- Les refus sont consignés dans `<data>/server/abuse.log` (heure, adresse, compte, type, détail), sans jamais de secret.
