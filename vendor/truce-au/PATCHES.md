# truce-au 6.3.0, patched for Sonder

Source : crates.io `truce-au` 6.3.0 (copie intégrale, premier commit de ce
dossier), fichiers de licence pris dans le dépôt amont
github.com/truce-audio/truce (Truce License 1.0 : MIT ou Apache-2.0 au choix,
plus un « Rider » qui ne vise que la revente de truce comme framework).
Utilisé via `[patch.crates-io]` dans le `Cargo.toml` du workspace.

## Patch 1 : redimensionnement AUv2 (2026-10-26)

Fichier : `shim/au_v2_view.m`, classe du conteneur AUv2.

Problème constaté dans Logic Pro (Vincent, 26/10) : en amont, le conteneur
NSView remis à l'hôte est figé à la taille d'ouverture de l'éditeur et
ignore tout `setFrameSize:`. Un `request_resize` du plugin agrandit la vue
de l'éditeur à l'intérieur d'un cadre fixe (contenu rogné), et les bords de
la fenêtre de Logic sont sans effet.

Correctif, actif seulement si l'éditeur est redimensionnable
(`gui_can_resize`, lu une fois avant `gui_open`) :
- le conteneur observe `NSViewFrameDidChangeNotification` de la vue de
  l'éditeur et prend sa taille : l'hôte, qui observe la vue du plug-in,
  redimensionne sa fenêtre ;
- un `setFrameSize:` de l'hôte est transmis à `gui_set_size` (bornes
  min/max/ratio appliquées par truce), puis le conteneur adopte la taille
  retenue ;
- `autoresizingMask` largeur/hauteur sur le conteneur ;
- Logic héberge les AUv2 hors processus (AUHostingServiceXPC) et ne voit
  que la vue parente du conteneur dans la fenêtre du service : un
  redimensionnement voulu par le plugin redimensionne cette fenêtre une
  seule fois (`setContentSize:`), la chaîne d'autoredimensionnement suit
  (même principe que le wrapper AUv2 de JUCE).

Éditeurs non redimensionnables : comportement amont inchangé.

Validé dans Logic par Vincent le 26/10 (zoom 75–200 % et poignée : la
fenêtre suit, pas de rognage). Limite : la fenêtre de service de Logic n'a
pas le style « resizable » ; tirer ses bords n'envoie aucun
`setFrameSize:` (journaux du 26/10), donc le redimensionnement par les
bords reste impossible en AUv2 dans Logic.

À proposer en amont (truce-audio/truce).

## Patch 2 : automatisation planifiée au bon échantillon (2026-10-26)

Fichier : `shim/au_v2_shim.c`, `au_v2_schedule_parameters` et
`enqueue_param_event`.

Problème mesuré en É8 (bounce Logic, EQ 2 On coupé à la mesure 3 temps 2,
trame 216 000) : le changement s'appliquait à la trame 215 040, début du
bloc hôte de 1024 qui la contient. En amont, chaque événement planifié est
à la fois écrit dans le magasin de paramètres (`param_set_value`) dès sa
réception, avant le rendu, et mis en file avec son décalage pour le
découpeur (`process_chunked`). L'écriture immédiate rend la nouvelle
valeur visible dès l'échantillon 0 du bloc ; le découpage au bon décalage
ne sert plus à rien.

Correctif : un événement de `ScheduleParameters` (immédiat ou rampe) qui
entre dans la file n'est plus écrit d'avance ; le découpeur l'applique à
son décalage. Il n'est écrit tout de suite que si la file (512) est pleine
et qu'il n'a pu être fusionné. `SetParameter` est inchangé (son
`bufferOffset` n'est pas fiable selon les hôtes, la valeur reste appliquée
au début du bloc).

Effet de bord : entre `ScheduleParameters` et le rendu suivant,
`GetParameter` renvoie l'ancienne valeur ; si l'hôte ne rend pas (bypass
Logic), les valeurs attendent le rendu suivant.

À proposer en amont (truce-audio/truce).

Voir aussi `vendor/truce-egui/PATCHES.md` (zoom egui).

## Correction de commentaire (2026-10-26)

Fichier : `src/lib.rs` (traitement des `param_events`). Le commentaire amont
disait que le chemin AUv2 passait `param_events = NULL` ; le shim v2
transmet en fait l'automatisation du bloc avec ses décalages (voir patch 2).
Commentaire seul, aucun changement de code.
