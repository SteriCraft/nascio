# Limites connues

Ce que le code fait mal ou ne fait pas encore, et que l'on a choisi de garder pour l'instant. Une entrée par limite : ce qu'on observe, d'où elle vient, ce qu'il faudrait pour la lever. Les travaux à faire sont dans les feuilles de route (`roadmap.md`, `roadmap-serveur.md`), qui renvoient ici par numéro.

Quand une limite est levée, on retire son entrée et on note la levée dans `decisions.md`. Les numéros ne sont pas réutilisés.

## Monde et génération

**L1. L'horizon ignore les éditions des joueurs.** Les tuiles d'horizon et le sol lointain montrent le monde généré : une montagne rasée reste visible au loin jusqu'au chargement de ses chunks. Les tuiles sont statiques, calculées une fois et gardées dans le cache disque du client. Pour la lever : invalider les tuiles par région éditée. Source : `decisions.md` (2026-11-18), spec §7.

**L2. Les porphyres de la graine 42 sont aux trois quarts sous la mer** (arcs insulaires). Depuis la version 20, leurs chapeaux de cuivre et l'uranium en front d'oxydation ne restent qu'à terre (contexte `land`), mais le cuivre profond reste rare à terre. C'est un trait de la tectonique de cette graine, pas un défaut de code. Source : `reports/gisements-surface.md`.

**L3. Le feuillage est déduit du nom du bloc** (`nascio_chunk::block::FOLIAGE`), pas déclaré dans `blocks.toml`. À corriger au prochain changement de format du monde (décision de l'auteur, 2026-11-18).

## Visualiseur et rendu

**L4. `DeviceLost` (Vulkan)** : vu une fois sur quatre lancements du visualiseur connecté, suivi d'une panique de wgpu-hal dans le swapchain. Relancer suffit. Cause non établie (pilote ou GPU). Source : `reports/chantier-5.md` §5.5.

**L5. Le panneau F3 n'a pas la souris pendant le jeu connecté**, puisque la souris est capturée. Échap la libère.

**L6. La vitesse de vol affichée (molette) n'est pas plafonnée** à `fly_max_speed` ; la physique, elle, la plafonne à 120 m/s.

**L7. Les règles des faces et les teintes des textures sont écrites dans le code**, pas dans un fichier TOML. Les textures de l'auteur sont hors du dépôt (`--textures <dossier>`). Sans dossier, les blocs gardent leur couleur.

**L8. Le coût des aurores n'est pas mesuré sur le portable Linux** (RTX 3050 Mobile), la machine cible ; seuls les bancs des sous-lots 5b et 5c y ont été refaits. Source : `rapport-audit-M7.md` §0 et §9 (constat R5).

## Réseau et serveur

**L9. Le statut d'administrateur n'est lu qu'à la connexion** (`ServerHello.admin`) : un `admin` donné en console ne vaut, pour la touche G du mode dieu, qu'à la reconnexion (la console `god` agit tout de suite).

**L10. Un client qui plante garde son compte 30 s.** Sans `Bye`, le serveur attend le délai d'inactivité de QUIC : « already connected » pour une reconnexion immédiate, et le serveur comble ses pas pendant ce temps.

**L11. Des connexions échouent pendant une arrivée en masse.** À 500 bots, 8 % des connexions dépassent les 10 s d'attente du `ServerHello`, même à 10 arrivées/s avec des comptes existants ; les échecs commencent vers 220 joueurs. Hypothèse non vérifiée : le hachage Argon2id de la connexion (au plus la moitié des cœurs) passe après la génération des colonnes. À mesurer avant un serveur public. Source : `reports/chantier-5.md`, mesure à 500 bots.

**L12. Avec `swarm`, le serveur n'applique qu'environ 85 % des pas** (235 000 par 10 s pour 276 000 attendus à 460 joueurs), alors que les bots envoient bien 60 commandes/s. Hypothèse : des commandes attendent une colonne que le serveur n'a pas encore chargée (bots à 4 m/s vers du terrain neuf). Les tests avec un vrai client (`prediction`, `journey`) ne montrent aucun écart. Source : idem.

**L13. `swarm` calcule sa position sans physique.** Quand le serveur arrête un bot contre le relief, les deux positions divergent, et une partie de ses éditions est refusée pour portée (5 à 9 m des yeux). C'est un défaut de l'outil, pas du serveur.

**L14. Le certificat est accepté au premier usage.** Le client épingle le certificat de la première connexion à une adresse (`known_servers.toml`). C'est suffisant en réseau local, pas pour un serveur public : il faudra par exemple une empreinte publiée par le site. Source : spec §8.7.

**L15. Le mot de passe est en clair dans `client-settings.toml`**, à côté de l'exécutable du client, enregistré après une connexion réussie. Les joueurs passeront par les jetons du site ; le secret reste surtout pour les administrateurs. Source : `reports/chantier-interfaces.md`.

**L16. Le client ne garde pas les colonnes sur le disque.** Seuls le ciel et les tuiles d'horizon sont en cache (`client-cache/`) : chaque connexion retélécharge les colonnes. Source : spec §8.3.

**L17. Le feuillage ajouré coûte surtout dans les ombres.** Sans ombres, son surcoût est négligeable. Avec ombres, sur un trajet en forêt, le rayon de découpe de 4 colonnes faisait tomber le visualiseur à 60 i/s (rayon 3 : à peine au-dessus de la référence), alors que la fréquence du GPU variait d'un essai à l'autre : mesure à refaire à fréquence fixe. Pour la lever : réduire le rayon (`[render] foliage_cutout_chunks`), ou projeter les ombres du feuillage sans découpe.

**L18. Une édition remaille des colonnes entières**, pas seulement le chunk 32³ touché (et ses voisins de bord). Mesuré sur world-42 : 0,37 ms par chunk, 1 à 3 ms par colonne sur un cœur ; le gain ne valait pas de ranger les maillages d'une colonne par chunk et de fusionner les requêtes qui se remplacent. Pour la lever : `Slot::Ready` par `cy`, et des requêtes de remaillage qui portent leurs `cy`. Source : `decisions.md` (2026-11-19).

## À savoir pour lire les mesures et déployer

- Les p50 et p99 des lignes du serveur portent sur les **60 dernières secondes** (1 200 ticks), pas sur 10 s : une mesure doit laisser au moins 60 s de régime établi après l'arrivée des joueurs (`swarm --ramp 10 --duration 90`).
- `swarm` et le serveur sur la même machine se gênent : les bots manquent de CPU, et le serveur fabrique lui-même la plupart des pas. Mesurer avec les bots sur une autre machine.
- Le serveur écoute par défaut sur `127.0.0.1:7777` et refuse `server_id = "dev"` (ou vide) hors adresse locale : un serveur public doit passer `--bind 0.0.0.0:7777 --server-id <nom>`.
