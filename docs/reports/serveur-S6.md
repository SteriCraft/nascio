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
