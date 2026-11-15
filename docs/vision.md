# Nascio — Vision

## Intention
Nascio est un jeu multijoueur en monde ouvert, fait de voxels de 1 m de côté, dans l'esprit de Minecraft et des modpacks techniques de Feed the Beast. Il vise un réalisme nettement plus poussé. Les administrateurs et les modérateurs interviennent le moins possible : ce sont les joueurs qui décident. Ils s'organisent en pays et choisissent librement leur structure politique.

## Le monde
- Un monde de **100 km × 100 km** dont les bords opposés se rejoignent (un tore). En allant toujours vers le nord, on finit par revenir par le sud ; il en va de même entre l'est et l'ouest.
- Le bord nord/sud, qui forme une seule ligne qu'on peut traverser, correspond aux hautes latitudes (les « pôles »). Le milieu de la carte correspond à l'équateur. Les latitudes sont réparties comme les surfaces d'une sphère (`latitude = asin(d)`) : les tropiques sont vastes et les zones polaires étroites.
- La géographie découle d'une simulation simplifiée de la tectonique des plaques (continents, chaînes de montagnes, planchers océaniques, archipels de points chauds), puis de l'érosion. La carte est ensuite figée.
- Le climat découle de la latitude, des saisons, des vents et des courants océaniques. Les biomes en découlent à leur tour.
- Le sous-sol est cohérent avec la géologie : les roches varient avec la profondeur et l'histoire tectonique, et les minerais se trouvent là où la géologie les place. La température augmente avec la profondeur jusqu'au **Moho**, qui marque l'entrée du manteau. Au-delà, il n'y a plus que du magme, et la mort est certaine.

## Le temps
- **1 journée = 1 heure réelle.** L'année compte 365 jours, soit environ 15 jours réels. Le temps du jeu suit l'horloge réelle.
- Il y a des **fuseaux horaires** : l'heure solaire locale dépend de la longitude. La frontière entre le jour et la nuit balaie la carte.
- Le soleil suit une course réaliste selon la latitude et la saison. Les transitions entre saisons sont continues.
- La ligne des pôles est une bande de **crépuscule permanent** : le soleil y reste sous l'horizon, des aurores apparaissent et il n'y a pas d'ombres.

## Le gameplay (plus tard)
- De vrais minerais, de vrais procédés de raffinage et un arbre technologique profond.
- Jouer seul n'est pas efficace : la coopération est nécessaire pour bâtir des sociétés complexes.
- L'organisation politique est libre : pays, gouvernements, diplomatie, guerres.

## Le site web (plus tard)
Il permet de suivre l'état du monde et des pays, de consulter son rôle, la politique, les relations internationales et les guerres, et propose un forum.

## Architecture générale
- Rust de bout en bout.
- Un serveur autoritaire qui génère, stocke et diffuse la carte.
- On ne stocke que la carte macro, figée, et les modifications faites par les joueurs. Les chunks sont régénérés à la demande.
