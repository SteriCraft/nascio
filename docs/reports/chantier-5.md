# Chantier 5 — Mouvement autoritaire, prédiction et réconciliation

Plan validé par l'auteur le 2026-11-18, avec ses réponses (`decisions.md`) : portée d'édition 5 m, course sur Maj, pas de montée automatique d'un bloc (saut automatique en option), vol sans traverser les blocs, mort par étouffement si un bloc solide est au-dessus de la tête. Étapes : 5.1 physique commune, 5.2 protocole v9, 5.3 serveur, 5.4 prédiction du client, 5.5 visualiseur, 5.6 tests d'intégration et documentation.

## 5.1 — Crate `nascio-physics`
- `PlayerParams` (TOML `[player]`, valeurs de l'auteur, `validate`), `Input` (touches, cap sur 16 bits, vitesse de vol), `State` (pieds, vitesse, au sol, dans un fluide, mode marche ou vol), `Shapes` (forme de collision par identifiant de bloc, tirée de la matière : plein, couche de neige de 0,125 m, fluide, vide), trait `BlockSource` (`None` = colonne non chargée).
- `step` : un pas de 1/60 s. Accélération horizontale vers la vitesse voulue (50 m/s² au sol et dans l'eau, 10 en l'air) ; saut, gravité, chute plafonnée ; nage (monter, plonger, couler à 1 m/s) ; vol sans gravité, plafonné à 120 m/s ; collision axe par axe ; marche de 0,5 m ; bond hors de l'eau contre une berge. Cellule inconnue : état inchangé, `Unloaded`.
- `unstuck` : remet un joueur enfermé sur les blocs qu'il chevauche, ou rend `Suffocated` si la rangée juste au-dessus de sa tête est solide (ou s'il resterait coincé).
- `auto_jump` : pour l'option du client ; vrai au sol devant une marche d'un bloc avec de la place au-dessus.
- 18 tests : chute et atterrissage exact, chute plafonnée, vitesses (marche, course, accroupi), arrêt contre un mur à sa face, bloc non franchi sans saut puis franchi en sautant, neige montée sans saut, hauteur du saut (1,25 m ± 0,1), plafond, nage, vol (collision, plafond de vitesse, descente jusqu'au sol), colonne non chargée, couture (mur de l'autre côté, passage est et ouest), poussée et étouffement, saut automatique, paramètres en TOML ; proptest : le même parcours au bord de la couture et 30 km plus loin donne le même mouvement (à 10⁻⁶ m) ; empreinte figée d'une trajectoire de 2 000 pas (même valeur en debug et en release ; à vérifier sous Linux).

Limites connues : la hauteur ne change pas accroupi, et l'on peut tomber d'un bord accroupi (pas de garde au bord) ; ~~le magma est un fluide où l'on nage~~ (erreur, corrigée en 5.3 : le magma est un bloc solide, on marche dessus ; sa mort au contact est faite en 5.3).

## 5.2 — Protocole v9
Plan validé par l'auteur (octet de type, `f64` exacts, 32 commandes, mouvement réseau figé jusqu'à 5.3).

- **Datagrammes typés** : un octet en tête. Serveur → client : `0` entités (format S5 inchangé derrière l'octet), `1` `PlayerState`. Client → serveur : `0` commandes.
- **`Commands`** (`crates/protocol/src/player.rs`) : type, n° de la commande la plus récente (varint), nombre (1 à 32), tangage (u8, pour la réplication), puis les commandes de la plus ancienne à la plus récente. La première est entière (touches, cap u16 LE, vitesse de vol en varint), les suivantes font un octet de touches ; le bit 7 annonce un varint `zigzag(Δcap) << 1 | vitesse de vol changée`, suivi de la nouvelle vitesse si elle a changé. Décodage strict (type, nombre, n° ≥ nombre − 1, bit 7 interdit en tête, changement vide, octets en trop).
- **`PlayerState`** : type, tick (varint), `ack + 1` (varint, 0 = aucune commande), drapeaux (au sol, dans un fluide, vol, masque des composantes de vitesse non nulles), position en trois `f64` LE, puis les composantes de vitesse non nulles (−0,0 est envoyé). Refus à l'encodage d'un état non fini ; au décodage, d'une valeur non finie, d'une position hors du tore, de drapeaux inconnus.
- **`InputQueue`** : la file du serveur (ordre, pas de doublon, trous sautés et comptés, file pleine sans perte).
- **Messages** : `ServerHello.physics` (`PlayerParams`) et `ServerHello.mode` ; `Move`, `Correct` et `max_speed_m_s` retirés ; `[player]` dans la configuration du serveur, validé au démarrage ; `input_datagrams_per_s` et `input_datagrams_burst` (40).
- **Tailles mesurées** (test `player::tests::sizes`, charge utile ; QUIC, UDP et IP ajoutent ≈ 55 o) :

| Datagramme | Octets |
|---|---|
| 9 commandes, au repos (une heure de jeu) | 18 |
| 9 commandes, en tournant à 90°/s | 34 |
| 32 commandes, pire cas réaliste | 165 (borne 231) |
| `PlayerState` au repos / en marche / en saut | 32 / 48 / 56 |

  Soit 1,5 à 1,8 Ko/s en montée et 1,7 à 2,2 Ko/s en descente par joueur, en-têtes compris.
- **Tests** (10 nouveaux, 29 dans `nascio-protocol`) : aller-retour des commandes (cap à travers 0, vitesse de vol, 32 commandes, toute troncature refusée) et de l'état au bit près (−0,0, valeurs extrêmes) ; refus ; file : pertes de 1 à 9 datagrammes d'affilée sans perte de commande, 11 perdus = trou compté, doublons et datagrammes en retard appliqués une fois ; proptest d'aller-retour et de pertes, doublons et retards quelconques (ordre strict, chaque commande appliquée ou comptée sautée).
- **Appelants** :
  - serveur : une tâche lit les datagrammes du client (seau de jetons, décodage, abus consignés `datagram` et `inputs`), remplit `PlayerLink::inputs` et reprend le cap et le tangage de la commande la plus récente pour la réplication. **Le serveur ne simule pas encore : le joueur reste à son point d'apparition (5.3).** La portée d'édition se mesure depuis `View::pos` ;
  - client : `Event::Player`, `Connection::send_commands` ;
  - `netbot --walk` devient un drapeau (marche en cercle depuis le point d'apparition) ;
  - `swarm` marche vers un but errant d'après le dernier `PlayerState` et compte les états reçus ;
  - visualiseur en mode réseau : commandes sans touche portant le cap et le tangage de la caméra.
- **Tests d'intégration** : `entities.rs` est ignoré jusqu'à 5.3 (le joueur ne bouge pas encore) et ses envois sont convertis en commandes ; `security.rs` remplace le test de téléportation par une avalanche de datagrammes de commandes (abus `inputs`).

## 5.3 — Simulation des joueurs par le serveur
Plan validé par l'auteur (protocole v10, crédit de pas, commandes fabriquées après 250 ms, magma mortel au contact, `SetBlock.cmd`, vol par la console et le client).

- **Corps** (`crates/server/src/movement.rs`) : état physique, crédit de pas (+3 par tick, 9 au plus, 4 pas par tick au plus), ticks sans commande ni datagramme, mort, installation à faire (`unstuck` à l'arrivée et à la réapparition), dernière commande, yeux des 64 dernières commandes.
  - Une commande dont une cellule est inconnue reste en file : le joueur attend, puis la rejoue telle quelle.
  - Après 5 ticks sans commande en file ni datagramme reçu (et après la première commande), des pas sans touche, numérotés à la suite (`InputQueue::fill`) et acquittés ; les vraies commandes de ces numéros sont ignorées ensuite.
  - Toucher du magma (1 cm) tue ; un mort vide sa file sans bouger (commandes acquittées).
- **Blocs de la simulation** (`world_view.rs`) : colonnes décodées des octets envoyés aux clients, lues comme `Mirror` (air au-dessus, inconnu au-dessous) ; construites par une tâche tokio (le tick n'attend jamais) ; demandées 2,5 m autour de chaque joueur à chaque tick ; éditions appliquées sur place ; `sim_columns` = 2 048 au plus (jamais une colonne utilisée au tick précédent).
- **Tick** (`sim.rs`) : entrées, puis éditions prêtes (ordre des joueurs), diffusion des blocs, mouvement de chaque corps (ordre des joueurs), puis pour chaque client son `PlayerState` et les entités, dans le même budget. `View::pos` vient de la simulation ; la tâche des commandes ne garde que le cap et le tangage. Les morts ne sont pas répliqués.
- **Éditions** : contrôlées dans la simulation. Portée de 5 m des yeux après la commande `cmd` au point le plus proche de la cellule ; une édition qui précède sa commande attend une seconde au plus ; pose d'un bloc qui fait obstacle refusée dans un joueur vivant (« inside a player ») ; refusée à un mort ; `unstuck` sur un joueur qu'une édition enfermerait (en pratique, la pose est refusée avant).
- **Protocole v10** : `Died { cause }` (`Suffocation`, `Magma`), `Respawned { pos }`, `Respawn`, `Fly(bool)`, `SetBlock.cmd`. `InputQueue::peek`, `fill`, `started`.
- **Vol** : `Fly` accepté d'un administrateur (vérifié à chaque demande, refus consigné `fly`), commande de console `fly <nom> on|off` ; marche à chaque connexion.
- **Configuration** : `edit_reach_m` vaut 5 (mesurée des yeux) ; `sim_columns` ajouté. Statistiques : pas (avec et sans commandes), morts, colonnes décodées et leurs octets, durée du mouvement (console `stats` et ligne toutes les 10 s).
- **Tests** :
  - unitaires (`movement.rs`, 5) : crédit et rattrapage, famine (figé 250 ms puis chute, acquittement des pas fabriqués, commandes tardives ignorées), cellules inconnues, magma, mort et réapparition, yeux et portée à travers la couture ; `InputQueue::fill` (protocole) ;
  - `tests/movement.rs` : 120 datagrammes (360 commandes : marche, course en tournant, sauts, recul, accroupi), 30 perdus (jusqu'à 3 d'affilée), 14 dupliqués, 10 en retard → **104 états du serveur égaux au bit près** à une simulation locale de `nascio-physics` sur les blocs reçus par le client ; aucun pas fabriqué ;
  - `tests/entities.rs` réactivé : le joueur court à 5,6 m/s en tournant et en sautant ; l'observateur (150 ms, gigue 30 ms, 5 % de pertes) le voit à 9 mm en moyenne du mouvement autoritaire (15 cm au pire), vitesse mesurée 5,60 m/s ;
  - `tests/rules.rs` : pose dans un joueur et hors de portée refusées ; arrivée dans deux blocs → posé dessus ; dans trois blocs → monté sur le dessus ; vol refusé à un joueur, accordé à un administrateur, retiré par la console ; `security.rs` : le refus de portée vient de la simulation (journal `reach`) ;
  - la mort (étouffement, magma), le refus d'édition au mort et la réapparition sont couverts par les tests unitaires de `movement` (aucune voie réseau ne peut plus enfermer un joueur : la pose est refusée).
- **Corrections trouvées par la mesure à 500 bots** (première passe) :
  - 17 comptes sur 500 mouraient à l'arrivée : `spawn::standing` pose le joueur sur le relief généré, sans les arbres, et certains points tombent dans un tronc (mort à chaque réapparition). À l'arrivée et à la réapparition, un joueur enfermé monte désormais à la première place libre (64 m au plus) ; l'étouffement reste pour l'enfermement par une édition. **À confirmer par l'auteur** (`decisions.md`).
  - Après une famine pendant la montée en charge, plus aucune commande des bots n'était appliquée (1,4 M pas fabriqués) : les datagrammes portaient des numéros déjà comblés, la file restait vide et le serveur continuait de combler, au rythme même du client. Correction : la famine se compte en ticks **sans aucun datagramme** (`InputQueue::pushes`), pas en ticks à file vide. Règle ajoutée en plus : un client qui reçoit un acquittement au-delà de sa dernière commande numérote la suivante après lui (swarm, netbot ; le client en 5.4), pour reprendre sans perdre l'écart.

**Mesure à 500 bots** (`swarm`, serveur et bots sur la même machine de 12 fils, `swarm` en priorité basse, `out/server/world-42`, chaque compte à son point d'apparition). Dernière passe, avec 500 joueurs connectés :

| | Tick p50 | Tick p99 | Mouvement p99 | Réplication p99 | Colonnes décodées |
|---|---|---|---|---|---|
| 500 joueurs | 3,8 à 7,4 ms | 25 à 34 ms | 2,8 à 4,7 ms | 18 à 31 ms | 626 colonnes, 128 Mo |

- Le mouvement de 500 corps (60 pas par seconde chacun, 30 000 pas/s) coûte **3 à 5 ms au p99** sur un seul fil : pas besoin de le répartir sur le pool (seuil prévu de 5 ms). Le p99 du tick reste dominé par la réplication (inchangée), dans le budget de 50 ms.
- Les colonnes décodées pour la simulation : 626 pour 500 joueurs répartis sur 10 sites, 128 Mo (≈ 200 Ko par colonne) ; le plafond de 2 048 colonnes tient ≈ 420 Mo.
- **Limite de la mesure** : `swarm` sur la même machine ne tient pas ses 20 envois par seconde (3 datagrammes de commandes par seconde et par bot en moyenne, trous jusqu'à 1,5 s ; il décode 360 000 colonnes) : le serveur comble les trous par des pas sans touche, comme prévu, et la plupart des pas sont fabriqués. Le coût mesuré reste le bon (un pas fabriqué fait la même physique), mais le chemin des vraies commandes à 500 joueurs se vérifiera avec les bots sur une autre machine (comme pour S6). Avec 10 bots, tous les pas viennent des commandes (600 pas/s, aucun fabriqué).
- Aucune mort, aucun abus sinon quelques refus de portée (bots affamés, qui visent depuis une position périmée).


Limites connues :
- Le client (`Mirror`) ajoute de l'air quand une édition tombe sous la plage d'une colonne, alors que le serveur redemande la colonne : prédiction fausse jusqu'au renvoi (creuser plus de deux chunks sous la surface la plus basse) ; à corriger en 5.4.
- Le visualiseur envoie `cmd: None` jusqu'en 5.5 : ses éditions sont mesurées depuis la position présente.
- Le passage marche ↔ vol n'est pas numéroté avec les commandes : une petite correction côté client au basculement.

## 5.3bis — Dernière position, expulsion, point obstrué
Plan validé par l'auteur. Pendant l'étape, l'auteur a écarté la protection des blocs autour des points d'apparition : un point obstrué est remplacé par un autre point tiré autour du même site (`decisions.md`).

- **Placement à l'arrivée** (`nascio_physics::free_place`, remplace `Body::arrive` et la montée de 64 m) :
  - une boîte libre sur place ne bouge pas ;
  - sinon, le joueur va à la place libre la plus proche, à 10 m au plus de ses pieds. Les candidats sont les centres de cellule, pieds au plancher ou sur une couche de neige, triés par distance, du plus haut au plus bas, puis par x et par z ;
  - une cellule inconnue compte comme prise ; le serveur attend toutes les colonnes à moins de 11 m (`WorldView::has_around`).
- **Sans place libre** :
  - un joueur revenu à sa dernière position meurt (étouffement) ;
  - un joueur à son point (première connexion, réapparition) reçoit un nouveau point tiré à 300 m au plus du même site, gardé dans son compte (`Spawns::redraw`), avec 8 tirages au plus ;
  - une édition qui enferme un joueur passe toujours par `unstuck` (le cas d'une cellule encore inconnue est réessayé au tick suivant, `Body::unstick`).
- **Dernière position** :
  - `Account.last` dans `accounts.toml`, en `f64` exacts ; `None` pour un joueur mort au départ, qui revient vivant au point de son compte ;
  - le `Hello.spawn` est le point d'arrivée ;
  - la position est gardée au départ, à l'arrêt propre (fin de `Sim::run`) et toutes les `position_save_s` (60 s).
- **Écriture** :
  - un fil `accounts` reçoit les changements de la simulation, les applique en mémoire et écrit le fichier au plus une fois par seconde ; une mutex d'écriture garantit que la dernière écriture est la plus récente ;
  - le départ part vers la simulation sous le verrou des sessions, et la simulation garde la position des joueurs partis : une reconnexion immédiate retrouve la position au bit près ;
  - **coût mesuré** (500 comptes, release) : fichier de 99 Ko, sérialisation 1,2 ms, écriture complète durable 4,2 ms en médiane (5,2 ms au pire), hors du tick.
- **Tests** :
  - physique (6 nouveaux) : boîte libre immobile, sortie d'un tronc par le côté, sortie d'une dalle épaisse par le côté, place sur la neige, rien de libre (roche, puis cellules inconnues) → `Nowhere`, la couture ;
  - `movement` : arrivée dans un tronc → à côté ; rien de libre → décision laissée à la simulation ;
  - `tests/arrival.rs` : reconnexion → même position au bit près ; position gardée chaque seconde en ligne ; après un arrêt propre, la position est lue dans `accounts.toml` au bit près ; un joueur enterré hors de toute plage meurt, part mort, puis revient vivant au point de son compte ;
  - `tests/rules.rs` : « monté sur le dessus » devient « expulsé à côté » (deux blocs, puis trois) ;
  - `tests/spawn.rs` : nouveau tirage autour du même site, ou autour du point fixe sans le garder.

Limites connues :
- Après un arrêt brutal, le joueur revient à sa position d'au plus 60 s avant.
- Dans une reconnexion plus rapide qu'un tick, le `Hello.spawn` peut être l'ancien point ; la simulation, elle, place le joueur au bon endroit, et le client se cale sur le premier état.
- ~~Le nouveau tirage d'un point obstrué n'est couvert que par les tests de `spawn`~~ (testé de bout en bout en 5.6, en posant les blocs dans les deltas du serveur avant la connexion).

## 5.4 — Prédiction et réconciliation du client
Plan validé par l'auteur. `crates/client/src/player.rs` ; le client dépend de `nascio-physics` et toujours d'aucune crate de génération (`tests/no_generation.rs`).

- **API** :
  - `Player::new(hello, shapes, now)` ;
  - `update(now, &Controls, &Mirror) -> Option<Commands>`, `on_state(&PlayerState, &Mirror)`, `on_msg(&ServerMsg)` ;
  - `state()`, `display_pos()`, `eye()`, `running()`, `last_cmd()`, `set_auto_jump(bool)`, `set_rtt(Duration)` ;
  - `events()` (`Died`, `Respawned`, `Mode`, `Snapped`) et `stats()` (corrections, commandes rejouées, renumérotations, états périmés).
- **Horloge** : pas fixes de 1/60 s sur l'`Instant` de l'appelant. Elle fait 0 à 4 pas par mise à jour, et 3 après un gel (le reste est jeté). Sa cadence varie de ±2 % pour garder la file estimée du serveur entre 2 et 6 commandes ; cette file est le minimum sur 2 s de (non acquittées − RTT × 60).
- **Réconciliation** :
  - un état n'est accepté qu'à tick croissant ; la prédiction repart toujours de l'état du serveur et rejoue toutes les commandes non acquittées ;
  - une correction est un changement au bit près de l'état prédit présent ;
  - un acquittement au-delà de la dernière commande vide la file, et la commande suivante prend `ack + 1 + ⌈RTT × 60⌉ + 3` : un aller-retour de pas d'avance sur ceux que le serveur comble encore (décision de l'auteur ; le serveur saute le trou).
- **Cellules inconnues** : la prédiction s'arrête sur la première commande qui en lit une et la retente à chaque pas. Le joueur est figé tant qu'elle est arrêtée, ou au-delà de 60 commandes non acquittées ; les répétitions continuent de partir.
- **Lissage** : affiché = prédit + décalage ; une correction ajoute (ancienne − nouvelle prédiction, sur le tore) ; décroissance τ = 100 ms, nulle sous 1 mm.
  - Recalage immédiat au-delà de 2 m, à la mort, à la réapparition, au changement de mode et à la renumérotation.
- **Commandes** :
  - envoi tous les 3 pas des 32 dernières commandes non acquittées ;
  - course : deux appuis sur Z en 300 ms, arrêt au relâchement ; accroupi = `CROUCH` ;
  - le saut automatique ajoute `JUMP` à la commande, que le serveur reçoit.
- **Mort** : le client continue de créer des commandes sans touche, sans prédire. Le serveur les acquitte sans effet et ne comble donc pas à la réapparition.
- **Protocole v11** : `ClientMsg::Resend { column }`.
  - Le `Mirror` oublie une colonne qu'une édition atteint sous sa plage (ses lots attendent ; une colonne plus ancienne arrivée entre-temps n'est pas gardée) et la redemande une fois (`take_resends`).
  - Le serveur la retire de `subs` et relance le parcours de l'anneau (`rescan`) ; elle repart sur le budget du client. Une colonne absente de `subs` est ignorée.
- **Tests** :
  - unitaires du client (7 nouveaux) : double appui, lissage et seuil de 2 m, état périmé ignoré, arrêt sur cellule inconnue puis reprise, rejeu sans correction et renumérotation, mort puis réapparition, gel (3 pas, le reste jeté), oubli et nouvelle demande d'une colonne dans `Mirror` ;
  - `tests/prediction.rs` : 150 ms de latence dans chaque sens, 30 ms de gigue, 5 % de pertes dans les deux sens.

| Scénario | Résultat |
|---|---|
| 8 s en cercle, en sautant, sans édition | **0 correction**, ≈ 4 400 commandes rejouées, aucun état périmé, cadence 1,00 |
| Mur posé 1,2 m devant, en marchant dessus | 1 correction, 0 dans les 1,5 dernières secondes, arrêté à 0,3 m du mur (au mm près) |
| Gel du client de 500 ms | 12 à 15 pas comblés par le serveur, 0 renumérotation, 4 à 5 corrections, 0 dans les 2 dernières secondes |
| Gel du client de 1 s | 42 à 45 pas comblés, **1 renumérotation**, 14 corrections, 0 dans les 2 dernières secondes (avec `ack + 1` : 7 à 8 renumérotations) |
| Mort (enterré) puis `Respawn` | `Died` puis `Snapped` ; `Respawned` puis `Snapped` ; 0 correction ensuite |

- **Renumérotation** : elle n'a lieu qu'après un gel plus long que 2 × latence + 300 ms, quand le serveur comble au-delà de la dernière commande du client. Sous ce seuil (500 ms à 150 ms de latence), les pas comblés restent derrière la numérotation du client : quelques corrections, sans renumérotation. Avec la règle du plan (`ack + 1`), le client se renumérotait à chaque état pendant un aller simple (7 à 8 fois) ; l'avance d'un aller-retour, validée par l'auteur, n'en laisse qu'une.

Limites connues :
- ~~Le visualiseur n'est pas encore branché sur `Player`~~ (fait en 5.5, avec `take_resends`).
- ~~`Respawned` arrive par le flux de contrôle, les états par datagrammes : un état d'avant la réapparition reçu après elle peut ramener la vue une fraction de seconde au lieu de la mort~~ (traité en 5.5).
- La mesure à 500 bots avec de vraies commandes (bots sur une autre machine) reste à faire.

Texte proposé pour la spec (§8.1, §8.4 et §8.5), mis à jour en 5.3 (**remplacé par la proposition de 5.6**, plus bas) :
- §8.1 : `PROTOCOL_VERSION` vaut 11 ; datagrammes non fiables pour les états des entités, les commandes du joueur et son état autoritaire, chacun précédé d'un octet de type. Le client dépend aussi de `nascio-physics`.
- §8.4 (nouveau point, **le mouvement**) : le client envoie à 20 Hz ses commandes numérotées, une par pas de 1/60 s, en répétant les 32 dernières non acquittées. Le serveur simule chaque joueur avec `nascio-physics`, contre les blocs mêmes qu'il envoie aux clients (air au-dessus d'une colonne, inconnu au-dessous) : chaque tick, au plus 3 pas plus un de rattrapage, dans un crédit de 3 pas par tick gardé jusqu'à 9 ; une commande dont une cellule est inconnue attend. Sans aucun datagramme de commandes pendant 250 ms, le serveur avance le joueur sans touche, numérote ces pas à la suite et les acquitte (les vraies commandes de ces numéros sont ignorées ; un client qui reçoit un acquittement au-delà de sa dernière commande numérote la suivante après l'acquittement). Il renvoie à chaque tick, avant les entités et dans le même budget, l'état du joueur (position et vitesse exactes, n° de la dernière commande appliquée), sur lequel le client rejoue ses commandes suivantes.
- §8.4 (suite, **arrivée, mort et vol**) : un joueur revient à sa dernière position (gardée avec son compte au départ, à l'arrêt du serveur et toutes les 60 s ; un joueur mort au départ revient à son point). À l'arrivée et à la réapparition, un joueur dans des blocs va à la place libre la plus proche à 10 m au plus ; sans place, il meurt d'étouffement à sa dernière position, ou reçoit un autre point près du même site s'il était à son point. Un joueur qu'une édition enferme est posé sur les blocs, sauf si la rangée au-dessus de sa tête est solide : il meurt d'étouffement. Toucher le magma tue. `Died { cause }` prévient le client ; un mort ne bouge plus et n'est plus vu des autres ; `Respawn` le ramène au point de son compte (`Respawned`). Le vol est réservé aux administrateurs (`Fly`, ou la commande de console `fly`), sans gravité, sans traverser les blocs.
- §8.4 (suite, **prédiction**) : le client prédit son joueur avec la même physique sur les blocs de son miroir, rejoue ses commandes non acquittées sur chaque état du serveur et lisse les corrections (τ = 100 ms, recalage au-delà de 2 m). Une édition sous la plage d'une colonne lui fait oublier la colonne, qu'il redemande (`ClientMsg::Resend`).
- §8.5 : « les positions (`Move`) et » disparaît ; le point « Un déplacement plus rapide… (`Correct`) » est remplacé par : le serveur simule lui-même le joueur, et un seau de jetons limite les datagrammes de commandes (`input_datagrams_per_s`). La portée d'édition (`edit_reach_m`, 5 m) se mesure des yeux, tels qu'ils étaient après la commande que porte `SetBlock.cmd`, au point le plus proche de la cellule ; une édition qui précède sa commande attend au plus une seconde ; un bloc qui fait obstacle ne se pose pas dans un joueur.

## 5.5 — Le visualiseur sur `Player`
- **Mode connecté** (`crates/viewer/src/player.rs`, `remote.rs`) : un `Player` est créé depuis le `Hello` et partagé entre le fil réseau et l'image (verrou du joueur toujours pris avant celui du miroir).
  - Le fil réseau lui passe chaque `PlayerState` (`on_state`, après `set_rtt` avec le RTT de QUIC), `Died` et `Respawned` (`on_msg`). Il vide `Mirror::take_resends()` en `ClientMsg::Resend` après chaque lot de blocs.
  - À chaque image, le système `walk` fait `update` avec les touches, envoie les `Commands` en datagramme, lit les événements et place la caméra aux yeux (`eye()`). L'ancien envoi de commandes sans touche disparaît.
- **Touches** : ZQSD (touches physiques WASD), Espace (sauter, monter en nageant ou en vol), Maj (s'accroupir, descendre). Deux appuis sur Z en 300 ms font courir ; le champ de vision passe de 42° à 60° (τ = 0,15 s). La souris tourne la vue, capturée pendant le jeu, libérée par Échap, le tchat et la mort. La molette règle la vitesse de vol.
- **Vol** (décision de l'auteur après 5.5, remplace la touche F proposée) : deux appuis sur Espace à moins de 300 ms envoient `ClientMsg::Fly`, **seulement si le compte est administrateur** (`ServerHello.admin`, **protocole v12**) ; champ de vision de 60° en vol comme en course. Sans changement de mode 2 s après, la vue affiche « Vol refusé par le serveur ». Le passage s'affiche « Vol » ou « Marche ».
- **Saut automatique** : case « Saut automatique » des paramètres, enregistrée dans `client-settings.toml` (`auto_jump`), décochée par défaut, passée à chaque image par `set_auto_jump`.
- **Éditions** (outil G) : en mode connecté, le rayon part des yeux et porte à 5 m ; `SetBlock.cmd = last_cmd()` ; aucune édition pendant la mort. Le vol libre local garde ses 20 m.
- **Mort** : voile gris, « Mort par étouffement » ou « Mort dans le magma », bouton « Réapparaître » (`ClientMsg::Respawn`, grisé en attendant la réponse) ; `Respawned` enlève le voile.
- **État tardif après une réapparition** (limite de 5.4, traitée) : à `Respawned`, le `Player` note le numéro de sa prochaine commande. Tant qu'aucun état n'acquitte une commande au moins aussi récente, il ignore les états (comptés « périmés »). Le serveur avait réapparu le joueur avant d'appliquer toute commande créée après la réception de `Respawned` : ces états-là viennent forcément d'après la réapparition. Test unitaire ajouté (`death_and_respawn_snap`).
- `--fly` est inchangé : sans `Remote`, ni `walk` ni le champ de vision n'agissent, et `fly_input` déplace la caméra comme avant.
- **Vérification en réseau** (serveur `--nogui` sur `out/server/world-42`, compte `c5test`, textures de l'auteur) : trois captures, caméra aux yeux sur l'herbe, au pied d'une marche de terre de 2 m, au-dessus d'une côte (≈ 350 m). Troncs, feuillage d'épicéa et sol sont visibles (le défaut vu à la fusion des textures, sol et troncs absents, ne se voit plus). Le serveur compte tous les pas avec leurs commandes, sans abus ni mort.
  - `out/c5-final/walk-1.png` (nord), `walk-2.png` (sud, la côte), `walk-3.png` (est, vers le bas : herbe, troncs, terrasses).
  - **Incident** : un lancement sur quatre s'est arrêté sur `DeviceLost` (Vulkan), suivi d'une panique de wgpu-hal dans le swapchain, avant la capture. Le même lancement, refait, est passé. Cause non établie (pilote ou GPU) ; à surveiller.

Limites connues :
- Le panneau F3 n'a plus la souris en mode connecté pendant le jeu (souris capturée) : ouvrir le menu (Échap) la libère.
- Un client qui plante n'envoie pas `Bye` : le serveur garde le compte 30 s (« already connected ») et comble ses pas.
- La vitesse de vol affichée (molette) n'est pas plafonnée à `fly_max_speed` ; la physique la plafonne.

## 5.6 — Tests de bout en bout et documentation
- **`crates/server/tests/journey.rs`** : un client complet (`Player` + `Mirror`, traités comme dans le fil réseau du visualiseur, `Resend` compris), contre un vrai serveur.
  - `a_client_walks_jumps_edits_dies_and_respawns` : il atterrit, marche 1,5 s vers l'est (6,6 m, **aucune correction**), saute sur place (pic mesuré aux images : 1,21 m, retour au même plancher au bit près, aucune correction). Une édition à 3,5 à 4,5 m des yeux est acceptée et arrive dans le miroir ; une à plus de 5,5 m est refusée (« out of reach »). Un second compte, revenu à une dernière position enfermée, meurt (étouffement), ne bouge plus mort, réapparaît sur `Respawn` et remarche. `hello.admin` est faux pour un compte nouveau.
  - `a_blocked_spawn_point_is_drawn_again` : le point d'un compte est le premier site ; avant sa connexion, une couche de granite sur deux est posée dans les deltas du serveur sur 23 × 23 × 24 m autour de lui (aucune boîte de 1,8 m ne tient à 10 m). Le joueur arrive à ce point (`hello.spawn`), le serveur ne trouve aucune place, tire un autre point autour du **même site** et le garde dans le compte ; le joueur y est, vivant, à plus de 11 m (53 m et 204 m selon les passes), sans aucune mort. Le nouveau tirage, testé jusqu'ici seulement au niveau de `Spawns`, l'est maintenant de bout en bout.
- **Campagne complète** (agent, release) : `cargo fmt --all -- --check` et `cargo clippy --workspace --all-targets -- -D warnings` sans remarque ; `cargo test --workspace --release` vert (33 binaires de test, aucun échec ; `prediction` 32 s, `journey` 7 s). `journey` relancé 3 fois : vert à chaque fois (marche 6,5 m, saut 1,21 m, nouveau point à 75, 215 et 220 m).
  - `write_cost_500_accounts` : `accounts.toml` de 99 Ko, sauvegarde complète 3,3 ms en médiane (2,8 à 4,0), sérialisation 1,4 ms.
  - `replication_cost_of_500_players_within_200_m` : 17,3 à 19,3 ms par tick en moyenne, meilleur tick 14,9 ms sur un fil, 2 133 o par client et par tick (inchangé).
- **Mesure à 500 bots avec de vraies commandes** : non faite, elle demande une seconde machine pour `swarm` (voir plus bas, « Reste à faire »).
- **Spec** : la proposition ci-dessous, validée par l'auteur, est **intégrée** à `world-spec.md` le 2026-11-19 (nouvelle §8.4bis Mouvement ; aussi `fly` dans la console §8.7 et les touches du jeu connecté dans la partie client).

### Proposition de texte pour la spec (validée, intégrée)
- **§8.1**, 2ᵉ point : « `PROTOCOL_VERSION` vaut **12** (…). Le client (`nascio-client`) ne dépend que de `nascio-core`, `nascio-chunk`, `nascio-physics` et `nascio-protocol`. » 3ᵉ point : « des datagrammes non fiables pour les états des entités, les commandes du joueur et son état autoritaire, chacun précédé d'un octet de type (serveur : 0 entités, 1 état du joueur ; client : 0 commandes). »
- **§8.2**, fin : « Le `ServerHello` donne aussi le point d'arrivée, la physique des joueurs (`PlayerParams`), le mode (marche ou vol) et si le compte est administrateur (`admin`, lu à la connexion). »
- **§8.4** (nouveaux points, après « Entités et débit », ou nouvelle section **8.4bis Mouvement**) :
  - **Commandes** : le client envoie à 20 Hz ses commandes numérotées, une par pas de 1/60 s (touches, cap sur 16 bits, vitesse de vol), en répétant les 32 dernières non acquittées. Le serveur les applique dans l'ordre, une fois chacune, et saute (en les comptant) celles qu'aucun datagramme ne porte plus.
  - **Simulation** : le serveur simule chaque joueur avec `nascio-physics`, contre les blocs mêmes qu'il envoie aux clients (air au-dessus d'une colonne, inconnu au-dessous) ; chaque tick, au plus 3 pas plus un de rattrapage, dans un crédit de 3 pas par tick gardé jusqu'à 9. Une commande qui lit une cellule inconnue attend. Sans aucun datagramme de commandes pendant 250 ms, le serveur avance le joueur sans touche, numérote ces pas à la suite et les acquitte ; les vraies commandes de ces numéros sont ignorées, et un client qui reçoit un acquittement au-delà de sa dernière commande numérote la suivante à `ack + 1 + ⌈RTT × 60⌉ + 3`.
  - **État** : à chaque tick, avant les entités et dans le même budget, le serveur envoie l'état du joueur (position et vitesse en `f64` exacts, au sol, dans un fluide, mode, n° de la dernière commande appliquée).
  - **Prédiction** : le client prédit son joueur avec la même physique sur les blocs de son miroir, rejoue ses commandes non acquittées sur chaque état et lisse les corrections (τ = 100 ms, recalage au-delà de 2 m, à la mort, à la réapparition, au changement de mode). Une édition sous la plage d'une colonne lui fait oublier la colonne, qu'il redemande (`ClientMsg::Resend`). Après une réapparition, il ignore les états qui n'acquittent pas encore une commande créée après elle.
  - **Arrivée, mort et vol** : un joueur revient à sa dernière position (gardée avec son compte au départ, à l'arrêt du serveur et toutes les 60 s ; un joueur mort au départ revient à son point). À l'arrivée et à la réapparition, un joueur dans des blocs va à la place libre la plus proche à 10 m au plus ; sans place, il meurt d'étouffement à sa dernière position, ou reçoit un autre point près du même site s'il était à son point (§8.6). Un joueur qu'une édition enferme est posé sur les blocs, sauf si la rangée au-dessus de sa tête est solide : il meurt d'étouffement. Toucher le magma tue. `Died { cause }` prévient le client ; un mort ne bouge plus et n'est plus vu des autres ; `Respawn` le ramène au point de son compte (`Respawned`). Le vol est réservé aux administrateurs (`Fly`, demandé par deux appuis sur Espace, ou la commande de console `fly`), sans gravité, jusqu'à 120 m/s, sans traverser les blocs.
- **§8.5** : dans le 1ᵉʳ point, « les positions (`Move`) et » devient « les datagrammes de commandes et » ; le 3ᵉ point (« Un déplacement plus rapide… `Correct` ») est remplacé par : « Le serveur simule lui-même chaque joueur ; un seau de jetons limite les datagrammes de commandes (`input_datagrams_per_s`, 40). La portée d'édition (`edit_reach_m`, 5 m) se mesure des yeux, tels qu'ils étaient après la commande que porte `SetBlock.cmd`, au point le plus proche de la cellule ; une édition qui précède sa commande attend au plus une seconde ; un bloc qui fait obstacle ne se pose pas dans un joueur vivant ; un mort n'édite pas. Une demande de vol d'un non-administrateur est refusée et consignée. »
- **§8.6**, nouveau point : « Un point d'apparition **obstrué** (aucune place libre à 10 m) est remplacé par un autre point tiré à 300 m au plus du même site, gardé dans le compte (8 tirages au plus, puis l'étouffement). Avec `spawn_xz`, le nouveau point est tiré autour du point fixe et n'est pas gardé. »

### Reste à faire pour clore le chantier
1. **Valider la proposition de spec** ci-dessus, puis l'intégrer à `world-spec.md`.
2. **Mesure à 500 bots, `swarm` sur une autre machine** (même commit sur les deux machines) :
   - serveur (cette machine) : `cargo run -p nascio-server --release -- out/server/world-42 --nogui --open-registration --bind 0.0.0.0:7787 --server-id c5-bots --data out/c5-bots/data` (autoriser le pare-feu de Windows ; copier `out/c5-bots/data/server/cert.der` sur l'autre machine) ;
   - bots (autre machine) : `cargo run -p nascio-tools --release --bin swarm -- <IP du serveur>:7787 <chemin>/cert.der --bots 500` ;
   - relever dans les lignes du serveur (toutes les 10 s, ou `stats`) : tick p50 et p99, movement p99, steps, « without commands » (attendu : proche de 0) ; paralléliser le mouvement s'il dépasse 5 ms.
3. Marquer le chantier 5 clos dans `docs/roadmap.md` après l'accord de l'auteur.
