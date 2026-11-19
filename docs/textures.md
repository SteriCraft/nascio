# Textures à faire

> Cases cochées : une image de test existe (dossier `textures_temp/blocks` de l'auteur, hors dépôt, copié par `scripts/sync-textures.ps1` dans `target/<profil>/textures`, que lit l'exécutable). Elles ne sont ni libres de droits ni dans `ASSETS.md` : à remplacer (voir `roadmap.md`).

## Règle de nommage (validée le 2026-11-19)
Un fichier PNG de 32×32 par texture, dans le dossier des textures, nommé d'après le bloc :
- `bloc.png` : toutes les faces ;
- `bloc_side.png` : les 4 côtés ;
- `bloc_top.png` : le dessus seul ; `bloc_bottom.png` : le dessous seul ;
- `bloc_end.png` : le dessus et le dessous (troncs, colonnes).

Priorité pour une face : `_top` ou `_bottom`, puis `_end`, puis `_side`, puis `bloc.png`. Les côtés prennent `_side`, sinon `bloc.png`. Exceptions écrites dans le code : le dessous de `grass` et de `podzol` est `dirt`, `snow_layer` prend `snow`.

Les blocs ont pour l'instant une couleur unie (`color` du registre). Cette liste recense les blocs qui attendent une texture. On la complète à chaque nouveau bloc. Chaque texture tierce va dans `ASSETS.md`, avec sa source et sa licence (CC0 de préférence).

## Surface et sols
- [x] `grass` : herbe (dessus), terre et herbe (côtés) ; texture en niveaux de gris, teintée selon le climat (couleur par sommet)
- [x] `moss` : mousse et lichens de toundra
- [x] `podzol` : litière d'aiguilles de la forêt boréale
- [x] `dirt` : terre
- [x] `red_soil` : sol rouge tropical
- [x] `mud` : vase (mangrove, marais, fonds de lac)
- [x] `sand` : sable de quartz
- [x] `gravel` : gravier

## Glace et neige
- [ ] `ice` : glace (glaciers, banquise), teintée de bleu
- [x] `snow` : neige
- [x] `snow_layer` : surcouche de neige (1/8 de bloc) (utilise `snow` pour l'instant)

## Altérites et roches
- [x] `regolith`, `laterite`
- [x] `sandstone`, `clay`, `limestone`, `gypsum`
- [ ] `granite`, `gneiss`, `schist`, `basalt`, `gabbro`, `andesite`, `granulite`, `peridotite`

## Végétation (provisoire)
- [x] `spruce_log` : tronc de sapin (dessus et côtés)
- [x] `spruce_leaves` : aiguilles de sapin (opaques pour l'instant)
- [x] `cactus`

## Fluides et divers
- [x] `water` : eau (teinte selon la profondeur, voir M6)
- [x] `magma`

## Minerais
- [ ] les blocs du catalogue des gisements (`config/deposits.toml`)
