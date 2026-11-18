# Rapport S7 — Comptes et sécurité (2026-11-18)

## Fait
- **Comptes** (`crates/server/src/accounts.rs`) :
  - un compte est un nom (3 à 16 caractères parmi lettres, chiffres et `_`) et un secret (8 caractères au moins) ;
  - le secret est gardé sous forme de **hachage Argon2id** (19 Mio, 2 passes : les paramètres par défaut de la crate), avec un sel aléatoire de 16 octets tiré de `ring` ;
  - chaque compte garde ses propres paramètres, pour pouvoir les durcir plus tard sans casser les comptes existants ;
  - le fichier `<data>/server/accounts.toml` est réécrit de façon durable à chaque création ;
  - la comparaison des hachages se fait en temps constant, et le hachage tourne sur un fil bloquant de tokio.
- **Poignée de main, protocole v4** :
  - `ClientHello` porte le nom et le secret, protégés par le chiffrement de QUIC ;
  - un nom inconnu crée son compte si `open_registration` (vrai par défaut) ; sinon, les comptes se créent avec la commande `adduser` de la console ;
  - un mauvais secret donne « wrong name or secret », sans dire lequel des deux est faux ;
  - « déjà connecté » n'est répondu qu'une fois le secret vérifié : un inconnu n'apprend pas qui est en ligne ;
  - après `login_failures_per_min` échecs (5) en une minute, l'adresse est bloquée jusqu'à la fin de la minute.
- **Limites par client** (`crates/server/src/security.rs`, des seaux de jetons) :
  - messages de contrôle : 200 par seconde, rafale de 400 ;
  - éditions : 20 par seconde, rafale de 40 ;
  - un message au-delà de la limite est ignoré, une édition au-delà est refusée (`EditRefused`) ;
  - après `kick_after_violations` refus (500), le client est expulsé.
- **Contrôles** :
  - **vitesse** : un `Move` plus rapide que `max_speed_m_s` (120 m/s, avec une marge d'une demi-seconde et de 2 m) est refusé, et le serveur renvoie le joueur à sa dernière position acceptée (nouveau message `ServerMsg::Correct`, que le visualiseur applique à la caméra) ;
  - **portée** : une édition à plus de `edit_reach_m` (160 m) de la position du joueur est refusée.
- **Journal des abus** (`<data>/server/abuse.log`, une ligne par événement : heure, adresse, nom, type, détail) :
  - types : `login`, `flood`, `edits`, `speed`, `reach`, `kick` ;
  - pour chaque type et chaque connexion, le 1er événement est écrit, puis un sur cent, avec son numéro ;
  - les secrets n'y figurent jamais.
- **Console d'administration**, sur l'entrée standard du serveur : `help`, `list`, `kick <nom> [raison]`, `stats`, `adduser <nom> <secret>`, `stop`. Les statistiques périodiques comptent les événements d'abus.
- **Clients** : `connect(addr, cert, name, secret)` ; le visualiseur a `--name` (`viewer`) et `--secret` (`viewer-dev-secret`) ; `netbot` et `swarm` ont des secrets de développement.

## Mesures et validation
- **Test `crates/server/tests/security.rs`** (serveur réel, limites resserrées). Abus refusés et journalisés :
  - mauvais secret ;
  - nom invalide ;
  - secret trop court ;
  - adresse bloquée après 3 échecs (même avec un bon compte) ;
  - deuxième session du même compte ;
  - édition à 101 m pour une portée de 16 m ;
  - 30 éditions d'un coup pour une rafale de 10 ;
  - téléportation de 5 km (refusée, avec le joueur remis à sa place, ce que confirme la position vue par le serveur) ;
  - 2 000 messages d'un coup (le client est expulsé, avec la raison « too many messages »).
  - Le test vérifie aussi l'expulsion d'un joueur par l'administrateur, et que le journal contient chacun des six types d'événement.
- **Test unitaire des comptes** : création, vérification, refus, persistance ; le fichier ne contient pas le secret.
- **Visualiseur** : le compte `viewer` est créé à la première connexion, et les colonnes arrivent comme avant (capture `out/s7-viewer.png`).
- **Coût d'Argon2id** avec les paramètres par défaut : quelques dizaines de millisecondes par connexion attendues, **non mesurées ici**. Le hachage tourne sur un fil bloquant, donc sans effet sur le tick.

## Décisions (détail dans `decisions.md`)
- **Argon2id**, avec la crate `argon2` (=0.6.0, nouvelle dépendance figée), et des sels tirés de `ring` (déjà présent par rustls, désormais figé =0.17.14).
- **Comptes dans un fichier TOML** du dossier de données, plutôt que dans une base de données. Cela suffit pour 2 000 comptes. Une base viendra avec le site web, s'il le faut.
- Le secret est envoyé dans le canal QUIC chiffré, et le certificat du serveur est épinglé. Pas de défi-réponse : TLS protège déjà le secret en transit.
- Les limites vivent sur le fil de lecture de chaque connexion, avant la simulation : un client abusif ne coûte rien au tick.

## Non décidé (gameplay ou produit), noté
- **Valeurs de jeu** : la vitesse maximale (120 m/s) et la portée d'édition (160 m) sont des réglages de développement qui laissent voler et éditer le visualiseur. Les vraies valeurs dépendent de la physique du joueur (voir S5).
- **Inscription** : ouverte (n'importe quel nom crée son compte), sur invitation (`adduser`), ou par le site web plus tard ? `open_registration` est vrai par défaut, pour le développement.
- **Sanctions** : aujourd'hui, uniquement l'expulsion. Bannissement durable ou temporaire, et par compte ou par adresse : à décider.

## Questions
1. Quel mode d'inscription veux-tu pour la première ouverture : ouverte, sur invitation, ou via le site ?
2. Les secrets suffisent-ils, ou faut-il prévoir dès maintenant une connexion par jeton émis par le site web (le client ne verrait jamais le mot de passe) ?
3. Faut-il un bannissement, et sur quoi (compte, adresse) ?
