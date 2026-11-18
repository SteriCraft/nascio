# Gisements manquants du catalogue (premiers échelons et nucléaire) — plan

Demande de l'auteur (prompt `gisements-prompt.md`, supprimé après la présentation de ce plan) :
- ajouter, **par le catalogue seulement**, le cuivre de surface, le fer de surface, le silex et le chert, l'uranium et le thorium ;
- signaler ce que le code ne sait pas exprimer ;
- proposer l'incrément de `generator_version` ;
- garder une rareté d'environ 0,1 % ;
- rendre le cuivre et le fer trouvables à pied ;
- vérifier la visibilité, le déterminisme et la couture ;
- vérification visuelle : carte de la graine 42, coupes, puis `stats` et `depths` avant et après.

**Statut : plan validé le 2026-11-18, avec les réponses suivantes. Rien n'est codé à ce stade.**
1. `generator_version` passe à 20.
2. Ajouter le champ `host_rock` et un bloc `chalk` (craie) dans la colonne sédimentaire ; le silex va dans le calcaire et la craie.
3. Ajouter le champ `above` : le chapeau de cuivre se place au-dessus d'un porphyre (`above = "porphyry_copper"`).
4. Générer dans `out/gisements/world-42`. Après validation de l'auteur, ce monde deviendra la référence et les deltas de `out/s3-data` seront jetés.
5. L'uranium des orogènes reste approximé (filons dans tout l'orogène, pas seulement dans les granites) : c'est noté dans `decisions.md`.
6. Garder les vérifications prévues et la mesure à 2 km. Un commit par sous-étape.

Sous-étapes prévues :
- G1 : `host_rock` dans le catalogue, plus le bloc `chalk` dans la colonne sédimentaire (marine, et plates-formes et bassins humides) ;
- G2 : `above` dans le catalogue, avec un placement au sommet d'un corps déjà tiré, dans `worldgen/deposits.rs` ;
- G3 : les huit types du catalogue, `generator_version` 20, un test de couture d'un manteau, la mesure à 2 km dans l'outil `geology`, la génération dans `out/gisements/world-42` et les vérifications visuelles ;
- un rapport (ce fichier, complété) et une ligne dans `decisions.md` par sous-étape.

## Ce que le code permet, et ce qu'il ne permet pas
Le catalogue choisit des contextes (tectoniques et climatiques) et une forme. L'hôte dépend de la **forme** et de la **classe** de roche, pas du type de roche :

| Forme | Remplace |
|---|---|
| `vein`, `stock`, `lens` | altérites, sédiments, socle |
| `layer` | sédiments (ou socle avec `host = "basement"`) |
| `blanket` | altérites seulement (régolithe ou latérite, environ 4 m sous le sol) |
| `placer` | sable et gravier, altérites, sédiments |

Le sol (herbe, terre) et la glace ne sont jamais remplacés.

Trois demandes dépassent le catalogue :
1. **Silex et chert « dans les calcaires et la craie »**. Le calcaire n'est qu'un banc parmi d'autres d'une colonne sédimentaire (grès, argile, calcaire, gypse), tiré par du bruit ; aucun contexte ne le désigne, et il n'existe pas de bloc de craie. Le catalogue peut seulement poser des rognons dans les sédiments des plates-formes et des plateaux, quel que soit le banc. Pour respecter vraiment l'hôte, il faudrait un champ `host_rock = ["limestone"]` qui restreint le remplacement à certains blocs : quelques lignes dans `voxel::geology::body_at` et le parseur du catalogue.
2. **Le chapeau de cuivre « au-dessus des porphyres »**. Le code tire les corps indépendamment ; aucun champ ne pose un corps au-dessus d'un autre. Le catalogue peut poser des chapeaux dans les mêmes contextes (arcs), donc dans les mêmes régions, mais pas exactement à l'aplomb d'un porphyre. Pour cela, il faudrait un champ `above = "porphyry_copper"` qui place le chapeau au sommet d'un corps déjà tiré, ce qui demande un changement du placement par tuile.
3. **L'uranium « dans les granites »**. Les plutons granitiques des chunks viennent d'un bruit du terrain et ne sont pas un contexte. Le catalogue peut poser des filons dans les orogènes, sans les restreindre aux granites.

**Ma recommandation** :
- pour cette étape, les approximations par le catalogue (ci-dessous) ;
- `host_rock` en petite étape séparée si tu veux le silex vraiment dans le calcaire : c'est la seule des trois qui change le ressenti du joueur, puisqu'un silex dans du grès se voit ;
- `above` peut attendre : les chapeaux d'arc tombent déjà dans les régions à porphyres.

## Types proposés
Les profondeurs se mesurent sous la surface rocheuse ; `abundance` compte les corps par 100 km² de contexte de poids 1. À titre de repère, la bauxite est à 96 et l'étain alluvionnaire à 288.

| Nom | Blocs (part) | Contextes | Forme | Profondeur (m) | Taille (m) | Épaisseur (m) | Teneur | Abondance |
|---|---|---|---|---|---|---|---|---|
| `copper_oxide` (chapeau oxydé) | malachite 0,55, azurite 0,15, cuivre natif 0,3 | arc 1 ; orogen 0,3 | blanket | 1–4 | 120–400 | 2–4 | 0,5 | 120 |
| `native_copper` (basaltes) | cuivre natif | rift 1 ; hotspot 0,6 | vein | 0–120 | 60–250 | 1–3 | 0,4 | 60 |
| `bog_iron` (limonite) | limonite | basin 1 ; platform 1 ; craton 0,3 ; requis : humid | blanket | 0,5–3 | 100–500 | 1–3 | 0,6 | 150 |
| `flint` (silex et chert) | silex | platform 1 ; basin 0,5 ; marine_shelf 0,5 | lens | 0–60 | 8–30 | 1–2 | 0,3 | 500 |
| `uranium_rollfront` (grès) | uraninite | basin 1 ; platform 0,2 | layer (sédiments) | 30–300 | 200–800 | 1–3 | 0,25 | 3 |
| `uranium_unconformity` | uraninite | craton 1 | layer (socle, sous la couverture) | 0–40 | 100–400 | 3–10 | 0,35 | 2 |
| `uranium_vein` (granites) | uraninite | orogen 1 | vein | 20–400 | 50–200 | 0,5–2 | 0,3 | 4 |
| `monazite` (sables côtiers) | sable à monazite | coast 1 | placer | 0,5–2 | 150–500 | 0,5–2 | 0,4 | 24 |

Nouveaux blocs, désignés par leur nom (couleur provisoire, densité en kg/m³) :

| Bloc | Couleur | Densité |
|---|---|---|
| malachite | vert [38, 138, 92] | 3 900 |
| azurite | bleu [40, 72, 160] | 3 800 |
| native_copper | cuivre [184, 112, 64] | 8 900 (pépites dans la roche : 4 000) |
| limonite | ocre [150, 100, 42] | 3 600 |
| flint | gris sombre [64, 64, 70] | 2 600 |
| uraninite | noir [34, 34, 30] | 7 500 |
| monazite_sand | brun-jaune [176, 140, 92] | 3 000 |

Éléments obtenus : Cu ; Cu (et Ag) ; Fe ; silex (outillage) ; U ; U ; U ; Th, Ce, La, Nd (terres rares).

Pourquoi ces valeurs :
- **cuivre et fer de surface** : les manteaux ne remplacent que les 4 m d'altérites, donc ils affleurent sous la terre et non en profondeur. Avec 120 à 150 corps par 100 km² de contexte, il y a environ un corps par km² de contexte favorable, soit quelques-uns dans un rayon de 1 à 2 km. Ils restent absents ailleurs : pas de cuivre de surface sur un craton ou une plate-forme sans rift, pas de limonite en climat sec ;
- **cuivre natif dans les rifts et les basaltes** (type Keweenaw) : il donne du cuivre aux continents sans arc ;
- **uranium rare et localisé** : les trois types font environ 9 corps par 100 km² de contexte au total, contre 38 pour le fer rubané ; ils sont petits et à faible teneur ;
- **silex abondant mais diffus** : nombreux petits rognons à faible teneur, il pèse peu en volume.

Je vérifierai le total avec `stats` : il doit rester proche de 0,1 % (0,114 % aujourd'hui).

## `generator_version`
Je propose **19 → 20** : le catalogue par défaut, les blocs du registre et les corps changent. Les mondes existants ne changent pas, puisque le catalogue est copié dedans. En revanche, `out/m7/world-42` (le monde de référence du serveur) ne serait plus régénérable à l'identique : je génère le monde d'essai dans `out/gisements/world-42` et je ne touche pas `out/m7` sans ton accord. Les deltas de `out/s3-data` restent liés à l'ancien monde.

## Vérifications
- **Avant et après** : `geology stats` et `geology depths` (graine 42), plus une mesure supplémentaire, la part des colonnes continentales à moins de 2 km d'un affleurement de cuivre et de limonite (petite option de l'outil `geology`, sans toucher à la génération).
- **Carte** des gisements de la graine 42 (couche `deposits`, export PNG), regardée et décrite.
- **Coupes** (`geology section`) à travers un chapeau de cuivre et une limonite, pour vérifier qu'ils sont sous le sol, ni effacés par les rivières, ni sous l'eau. Une coupe à travers un filon d'uranium et un placer de monazite si possible.
- **Déterminisme** : le test de hachage couvre le catalogue et les corps, donc les nouveaux types. **Couture** : les formes existantes ont déjà leurs tests de continuité ; j'ajoute un test qui pose un `blanket` (cuivre) à cheval sur la couture et compare les blocs des deux côtés.
- `world-spec.md` §4.6 ne liste pas les types, donc pas de mise à jour, sauf les champs ajoutés si tu veux `host_rock`. Une ligne dans `decisions.md`.
- Commit, puis push après ta validation.

## Questions
1. `generator_version` 20 : d'accord ?
2. Le silex : approximation dans tous les sédiments de plate-forme, ou petite étape `host_rock` (calcaire seulement) d'abord ? Faut-il un bloc `chalk` (craie) ?
3. Le chapeau de cuivre : approximation par contexte (arcs), ou `above = "porphyry_copper"` ?
4. Régénérer `out/m7/world-42` (le monde du serveur) en v20, ou garder un dossier séparé jusqu'à nouvel ordre ?

## G1 — `host_rock` et la craie (fait)
- Bloc `chalk` en fin de table des blocs ; bancs de craie dans les sédiments marins et de plate-forme humide (parts dans `decisions.md`).
- Champ `host_rock` du catalogue : il restreint le remplacement à certaines roches, en plus du filtre de la forme ; `voxel::geology` passe la roche remplacée à `body_at`.
- `generator_version` 20, valeurs dorées mises à jour ; test du catalogue `host_rock_is_optional_and_checked`, test de la colonne avec la craie.

## G2 — `above` (fait)
- Champs `above` et `probability` du catalogue : un corps au plus par corps parent, avec la probabilité `probability` × le poids des contextes, centré sur le parent ; validations (parent antérieur, avec des blocs, sans `above` ; probabilité dans ]0, 1] ; pas d'abondance, ni placer ni carte seule).
- `worldgen::deposits` : `shape_body` (mise en forme d'un corps, extraite sans changer l'ordre des tirages : valeurs dorées inchangées) et `above_bodies`.
- Tests : `above_names_an_earlier_type_with_a_probability` (catalogue) ; `crates/worldgen/tests/deposits_above.rs` (monde au 1/25 : chaque chapeau est sur un porphyre, les autres types ne bougent pas, déterminisme).
