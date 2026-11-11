# truce-egui 6.3.0, patched for Sonder

Source : crates.io `truce-egui` 6.3.0 (copie intégrale, premier commit de ce
dossier) ; licence : voir `LICENSE` (Truce License 1.0, MIT ou Apache-2.0).
Utilisé via `[patch.crates-io]` dans le `Cargo.toml` du workspace.

## Patch 1 : prise en compte du `zoom_factor` d'egui (2026-10-26)

Fichier : `src/editor.rs`.

Le `zoom_factor` d'egui multiplie les pixels par point. L'intégration amont
passait à egui la taille de la fenêtre et la position du pointeur en points
de fenêtre, sans les diviser par le zoom. Effets constatés dans Logic par
Vincent : contenu rogné à 200 %, trop petit et mal calé à 75 %, clics
décalés par rapport aux boutons. Correctif : `screen_rect`, `PointerMoved`
et la molette divisés par `zoom_factor()`. Validé dans Logic le 26/10.

À proposer en amont (truce-audio/truce).

## Patch 2 : rendu plus tôt après un redimensionnement sous macOS (2026-10-26)

Fichier : `src/editor.rs`, constante `RESIZE_SETTLE`.

Problème signalé par Vincent dans Logic (É7, 26/10) : après un zoom ou un
relâchement de la poignée, l'ancienne image reste affichée étirée environ
0,5 s (flou), puis le rendu net apparaît. Cause : après un
redimensionnement, truce-egui attend que la taille soit stable depuis
`RESIZE_SETTLE` (300 ms) avant de repeindre, pour éviter des blocages du
thread graphique de l'hôte pendant un redimensionnement continu (mesurés
sous Windows).

Correctif : 50 ms sous macOS, 300 ms ailleurs (inchangé). Dans Logic, une
fenêtre AUv2 n'est jamais redimensionnée en continu par l'hôte (bords
inopérants, ADR 0003) : chaque redimensionnement vient de l'éditeur
(boutons de zoom, poignée au relâchement), une seule fois. 50 ms laissent
à AppKit le temps de finir sa mise en page avant le premier rendu.

À valider dans Logic (ADR 0004, É1). À proposer en amont avec le patch 1.

## Patch 3 : les touches non utilisées reviennent à l'hôte (2026-10-28, É11c)

Fichiers : `src/editor.rs` (fonction `key_goes_to_host`, champ
`wants_keyboard`, branche `Event::Keyboard`).

Problème signalé par Vincent dans Logic (É10, 28/10) : quand l'éditeur a
le focus, Cmd+Z n'annule rien (le bouton Undo de Logic fonctionne). Cause :
l'intégration renvoie `EventStatus::Captured` pour tout événement clavier,
donc baseview ne le passe jamais à la superclasse de sa vue, et la chaîne
des répondeurs d'AppKit (la fenêtre de Logic) ne le voit pas.

Correctif : si aucun champ texte d'egui n'avait le clavier à la fin de la
frame précédente (`egui_wants_keyboard_input`), l'événement est rendu à
l'hôte (`Ignored`) sans être transmis à egui. Pendant la saisie d'une
valeur ou d'un nom de preset, egui garde toutes les touches. L'éditeur de
Sonder n'a pas de raccourci clavier propre. baseview fait passer `keyDown`,
`keyUp` et `flagsChanged` par le même chemin : la règle est symétrique, et
aucune touche ne reste enfoncée d'un côté (seul cas limite : le focus qui
change entre l'appui et le relâchement, sans effet pratique).
`performKeyEquivalent` n'est pas surchargé par baseview, donc AppKit le
propose déjà à la fenêtre de l'hôte ; s'il le faut après le test dans
Logic, le seul complément autorisé est de le laisser passer quand egui n'a
pas le clavier. Test : `mastering-plugin/tests/keyboard.rs`.

Validé dans Logic le 28/10 par Vincent : barre d'espace et saisie de texte
OK. **Limite connue** : Cmd+Z et Cmd+Maj+Z ne remontent toujours pas (les
équivalents clavier passent par `performKeyEquivalent`, dans la vue de
baseview, non vendorée). Décision du coordinateur : pas de patch baseview
pour l'instant, suivi non bloquant pour la fin de phase 2 ou plus tard. Les
boutons Undo / Redo de Logic et de Sonder restent disponibles. À proposer en
amont avec les patchs 1 et 2.
