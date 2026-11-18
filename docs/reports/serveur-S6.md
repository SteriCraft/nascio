# Rapport S6 — Charge (2026-11-18)

## Fait
- **Bin `swarm`** (`crates/tools/src/bin/swarm.rs`) : N bots dans un seul processus, connectés à 50 par seconde, puis répartis dans un disque de 200 m de rayon autour du point d'apparition.
  - Chaque bot marche à 4 m/s et envoie `Move` à 20 Hz.
  - Il décode et acquitte les datagrammes d'entités.
  - Tous les bots ensemble posent ou retirent 20 blocs par seconde.
  - Mesures sur une fenêtre fixe : octets reçus par bot (colonnes, entités), datagrammes par seconde, entités vues, bots qui ne voient personne, éditions refusées, erreurs.
- **Corrections et optimisations du serveur, trouvées par la charge** :
  1. **État de réplication par client** : un vecteur trié par identifiant, fusionné en une passe avec les voisins, au lieu de quatre `BTreeMap` consultées pour chaque paire. Les 64 plus prioritaires sont choisis par `select_nth`. Coût sur un fil pour 500 joueurs à moins de 200 m : 15 à 19 ms par tick, stable sur 2 000 ticks (test ignoré `replication_cost_of_500_players_within_200_m`).
  2. **Réplication en parallèle, un client par tâche**, dans un pool dédié (`replication_threads`, 4). Le résultat reste déterministe : les datagrammes d'un client ne dépendent que de son propre état.
  3. **La simulation ne touche plus aux verrous des connexions QUIC.** Les datagrammes passent par un canal à une tâche réseau par client. Celle-ci tient à jour la place libre du tampon de datagrammes : après chaque envoi, et toutes les 25 ms pour qu'un client sauté ne reste pas bloqué.
  4. **Contre-pression** : le tampon de datagrammes est réduit à 4 ticks (quinn met 1 Mo par défaut, soit des secondes d'états périmés). Un client dont le lien est plein est sauté pour ce tick, et ses états suivants seront plus récents.
  5. **Bug corrigé** : un premier datagramme plus gros que ce que le chemin permet (taille encore inconnue) faisait échouer `send_datagram`, et l'envoi s'arrêtait pour toujours. Des bots arrivés en dernier ne voyaient alors aucune entité. Désormais, la taille est connue dès la connexion, et un datagramme trop gros est perdu sans rien arrêter.
  6. **Envoi des colonnes** : le flux d'un joueur ne reparcourt plus son anneau de colonnes (environ 800 recherches toutes les 50 ms) tant qu'il ne change pas de colonne et que l'anneau est complet. Le serveur est passé de 3,4 à 2,8 cœurs pour 500 joueurs.
- Le journal du serveur affiche à part le p99 de la réplication et le nombre de clients sautés.

## Mesures
Ryzen 5 5500 (6 cœurs, 12 fils), Windows. **Le serveur et `swarm` tournent sur la même machine.** Monde de référence, 500 bots, fenêtre de 90 s.

| | tick p50 | tick p99 | réplication p99 | par bot reçu (p50 / p99 / max) | entités vues (min / moy.) |
|---|---|---|---|---|---|
| avant optimisation | 80 ms | 122 ms | — | 14,8 / 18,1 / 26,3 Ko/s | 286 / 393 |
| final, `swarm` en priorité normale | 6 à 40 ms | ≤ 49,4 ms | ≤ 40 ms | 27,3 / 33,5 / 34,2 Ko/s | 283 / 388 |
| final, `swarm` en priorité basse | 4,5 à 35 ms | ≤ 42,6 ms | ≤ 33,7 ms | 27,4 / 33,9 / 34,2 Ko/s | 283 / 387 |
| final, 250 bots | 6,5 ms | 15 à 21 ms | ≤ 14,6 ms | 17,0 / 20,0 / 20,3 Ko/s | 142 / 197 |

- **Critères du jalon** : tick p99 < 50 ms, **atteint** dans toutes les mesures finales ; débit par client ≤ 128 Ko/s, **atteint** (34 Ko/s au plus).
- **Le serveur** :
  - envoie 110 Mbit/s au total à 500 joueurs, dont 92 % d'entités. Le modèle de S0 prévoyait 165 Mbit/s pour 41 Ko/s par client ;
  - utilise environ 970 Mo de mémoire vive et 2,8 cœurs ;
  - n'a refusé aucune édition (2 290 éditions) et ne rapporte aucune erreur.
- **Datagrammes** : 28 par seconde et par bot. Avec 500 bots, environ 10 % des ticks d'un client sont sautés parce que son lien est plein ; avec 250 bots, environ 1 %.

## Ce qui reste à éclaircir
- **Un changement de régime, sur cette machine seulement.** Environ 60 à 70 s après le démarrage, le p50 du tick passe de 7 ms à 35-40 ms. La réplication isolée ne ralentit pas sur 2 000 ticks, et le CPU du serveur reste stable. À ce moment-là, `swarm` passe de 2 à 3,7 cœurs, et les deux processus dépassent alors les 6 cœurs physiques. Les fils du pool de réplication attendent leur tour d'ordonnancement (de l'ordre du quantum de Windows, environ 20 ms), et le tick, qui attend le plus lent de ces fils, s'allonge d'autant.
- **À refaire** sur un serveur Linux seul, avec les bots sur une autre machine. C'est l'hypothèse d'hébergement de la roadmap. Je n'ai pas pu le faire ici.

## Décisions (détail dans `decisions.md`)
- Pool de réplication dédié de 4 fils, plutôt que le pool global de rayon (qui sert aussi à la génération) ou un fil par client.
- La simulation ne parle pas aux connexions : elle passe par un canal, avec un compteur d'octets en attente et la place libre du tampon QUIC tenue à jour par la tâche réseau.
- Tampon de datagrammes de 4 ticks, et client sauté quand son lien est plein, plutôt que de laisser QUIC accumuler puis jeter des états périmés.

## Questions
1. As-tu une seconde machine, ou un serveur Linux, pour refaire S6 avec les bots séparés ? C'est la seule façon de mesurer le serveur seul.
2. 500 joueurs dans 200 m, c'est l'hypothèse de bataille de la roadmap. Faut-il viser plus, par exemple 1 000 dans la même zone ? Au-delà, il faudrait réduire `entity_fast` ou le rayon, ou ajouter un niveau de détail pour les entités lointaines.

## Mesure sur deux machines (2026-11-18)
Procédure : `serveur-S6-deux-machines.md`. Le serveur tourne sur le portable Linux de l'auteur : Ryzen 5 5600H (6 cœurs, 12 fils), 13 Gio, Ubuntu 24.04. `swarm` tourne sur le PC Windows (Ryzen 5 5500). **Liaison en Wi-Fi**, aucune liaison câblée possible. Monde de référence, fenêtre de 300 s.

| | joueurs | tick p50 | tick p99 | réplication p99 | par bot reçu (p50 / p99 / max) | datagrammes / bot / s | clients sautés |
|---|---|---|---|---|---|---|---|
| 500 bots | 428 | 7,3 à 7,8 ms, stable | 11,6 à 14,3 ms | 7,5 à 7,9 ms | 18,3 / 21,3 / 22,2 Ko/s | 16,4 | ~25 % des envois |
| 250 bots | 248 | 5,0 → 5,65 ms en 3 min, puis plat | 6,8 à 7,1 ms (10,6 au plus) | 4,4 ms | 16,4 / 17,7 / 18,1 Ko/s | 19,8 | seulement pendant la montée en charge |

- **Plus de changement de régime.** Le tick médian reste stable sur 5 minutes dans les deux mesures. La dérive de 7 à 35-40 ms venait bien de la cohabitation du serveur et de `swarm` sur la même machine.
- **Critères du jalon atteints** : tick p99 < 50 ms (14,3 ms au pire) ; débit par client ≤ 128 Ko/s (22 Ko/s au plus).
- **CPU, à 250 bots** : environ 1 cœur pour le serveur. Le portable passe de 1,5 % au repos à environ 9,5 % pendant la mesure (`mpstat`, 12 fils), et aucun cœur ne sature. `pidstat` avait suivi un serveur arrêté entre-temps, donc ni le CPU ni la mémoire du processus n'ont été mesurés en charge.
- **Réseau, à 250 bots** : 41 Mbit/s en sortie (32 Mbit/s de charge utile mesurés par `swarm`), 14 700 paquets par seconde en sortie, 12 400 en entrée. Aucune perte sur l'interface. En revanche, des erreurs de réception UDP du noyau apparaissent pendant la rafale des connexions (75 par seconde pendant 10 s).
- **À 500 bots, le Wi-Fi sature** (environ 63 Mbit/s utiles). Un quart des envois est sauté parce que le lien est plein, et des coupures provoquent des événements `flood` : le client renvoie d'un coup les messages accumulés. Le serveur, lui, tient : son tick est le même qu'à 250 bots, à 2 ms près.
- **Connexions perdues pendant la montée en charge** : 72 sur 500, puis 2 sur 250 (`bot160`, `bot170` : « handshake timed out »). Le serveur avait bien reçu leur connexion, mais il a répondu trop tard. C'est le moment où le tampon UDP déborde et où les hachages Argon2id se cumulent.
- Les événements `speed` du journal venaient de `swarm`, qui téléportait chaque bot de son point d'apparition à sa place dans le disque.

### Corrections après la mesure
1. **Tampons UDP du serveur** : 4 Mio demandés en réception et en émission (`udp_buffer_bytes`). Le serveur crée lui-même sa socket, avec `socket2` =0.6.5, déjà présent dans les dépendances de tokio. Si le système accorde moins, le serveur l'affiche au démarrage. Sous Linux, le plafond est `net.core.rmem_max` : le réglage `sysctl` de la marche à suivre ne servait à rien tant que le serveur ne demandait pas plus.
2. **Hachages limités** : au plus `account_hash_concurrency` hachages Argon2id à la fois, la moitié des cœurs par défaut. Une rafale de connexions attend son tour au lieu d'occuper toute la machine.
3. **Rafale après une coupure** : au-delà de la limite de messages, les `Move` et les `Ack` sont jetés sans compter pour l'expulsion. Le `Move` suivant remplace les précédents, et les acquittements ne sont qu'indicatifs. Ces événements sont journalisés sous le type `burst`. Les autres messages restent comptés : une inondation de `Ping` est toujours expulsée.
4. **`swarm`** : un bot part du point d'apparition et marche jusqu'à sa place à 80 m/s, sous la limite du serveur. Il applique `Correct`, et le bilan compte les corrections. En local, 50 bots : 0 événement `speed`, 0 correction.
5. Le test `security.rs` couvre la rafale : 1 000 `Move` et 1 000 `Ack` d'un coup, client toujours connecté. Il couvre aussi la file des hachages (`account_hash_concurrency` = 1).

### Reste
- Refaire 250 bots avec ces corrections : les connexions perdues pendant la montée en charge doivent disparaître. Lancer `pidstat` **après** le serveur.
- 500 bots demandent une liaison câblée, ou un second PC de bots sur un autre point d'accès.
