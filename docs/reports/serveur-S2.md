# Rapport S2 — Protocole (2026-11-18)

## Fait
Nouvelle crate `crates/protocol` (`nascio-protocol`), sans réseau. Elle dépend de `nascio-core`, de `nascio-voxel` (pour `Chunk`), de `serde`, `postcard` et `zstd`.
- `chunk` : **codec v2**. Octet de version 2, palette en varints, puis plages varint `(indice, longueur − 1)` dans l'ordre des indices ; pas de plages si la palette n'a qu'une entrée (chunk uniforme en 3 à 5 octets). `Column` : les chunks d'une colonne à partir de `cy_min`, avec un numéro de version (compteur d'éditions, clé du cache client), chacun préfixé de sa longueur, le tout compressé en zstd-3. Garde-fous : 16 Mio décompressés au plus, 512 chunks, identifiants bornés par la table de la session.
- `messages` : `ClientHello`, `ServerHello` (version du protocole, `generator_version`, empreinte, table des blocs, heure UTC du serveur, identifiant du joueur, point d'apparition, rayon des colonnes), `ClientMsg` (`Ping`, `SetBlock`, `Move`, `Have`, `Bye`) et `ServerMsg` (`Refused`, `Pong`, `Blocks`, `EditRefused`, `Forget`, `Kick`), sérialisés avec `postcard`. Une édition tient en 9 octets.
- `frame` : trames « longueur varint + contenu » et `Deframer` qui reconstitue les trames depuis des morceaux quelconques ; taille maximale refusée au-delà.
- `entity` : états quantifiés (1/64 m ; lacet sur 16 bits ; tangage sur 8 bits), en différence par rapport à un état de référence confirmé par le client, sinon complets ; entités retirées. Les différences de x et z passent par `torus::delta` sur la partie en blocs (invariant 1). Un petit déplacement qui traverse la couture tient en 7 octets.
- `clock` : décalage de l'horloge du client à la manière de NTP (on garde la sonde au plus court aller-retour). Le temps du jeu reste dérivé de l'UTC (invariant 6).
- `varint` : LEB128 et zigzag, lecture bornée.
- `Chunk::as_dense` ajouté à `nascio-voxel` (lecture du tableau sans copie).
- 14 tests, dont 7 `proptest` : allers-retours des chunks, des colonnes à travers la couture (cx, cz = 0 et 3 124), des trames coupées n'importe où, des instantanés d'entités ; entrées tronquées ou malformées refusées.

## Mesures (`netbench codecs out/m7/world-42 --sites 48`)
- Colonne v2 compressée : **1 290 o sur terre, 481 o en mer**. S0 prévoyait 1 277 et 468 o : l'écart de 1 % vient de l'en-tête et des longueurs.
- Encodage v2 : 25 µs par chunk non uniforme, copie de test de 64 Kio comprise (v1 : 21 µs).
- Aller-retour complet d'une colonne (encodage, compression, décompression, décodage) : 1,58 ms sur un fil. C'est une borne haute : `zstd::bulk` recrée son contexte à chaque appel. Le serveur réutilisera un compresseur (S3).

## Décisions (détail dans `decisions.md`)
- `postcard` pour les messages de contrôle ; formats écrits à la main pour les chunks et les entités.
- Le codec v2 vit dans `nascio-protocol` ; le codec v1 de `Chunk::encode` reste dans `nascio-voxel` pour l'instant, sans utilisateur réseau.
- La crate du protocole dépend de `nascio-voxel` pour le type `Chunk`. Le client y aura donc accès au code de génération, sans les données du monde.
- Quantification des entités : 1/64 m.

## Questions pour l'auteur
- **Le client lie `nascio-voxel`**, qui contient aussi la génération. L'invariant 5 est respecté (le client n'a pas les données macro et n'appelle pas la génération), mais on peut séparer les types de chunks dans une crate à part si vous voulez une garantie à la compilation. C'est un choix d'architecture qui touche un invariant : je ne l'ai pas tranché.
- **Précision des entités** (1/64 m, lacet au 1/65 536 de tour) : valeur par défaut technique ; à confirmer avec la physique du joueur en S5.
