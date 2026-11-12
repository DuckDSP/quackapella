# truce-clap 6.3.0, patched for Quackapella

Source : crates.io `truce-clap` 6.3.0 (copie intégrale, premier commit de ce
dossier), fichiers de licence repris de `vendor/truce-au` (même dépôt amont
github.com/truce-audio/truce). Utilisé via `[patch.crates-io]` dans le
`Cargo.toml` du workspace.

## Patch 1 : `latency.changed` seulement pendant `activate` (2026-11-12)

Fichier : `src/lib.rs`, `clap_plugin_activate` et `clap_plugin_on_main_thread`.

Problème signalé par clap-validator : quand un réglage change la latence
pendant que le plug-in est actif (Style, Voice, Low Latency dans
Quackapella), le wrapper appelait `clap_host_latency::changed` depuis
`on_main_thread`, puis `request_restart`. La spécification CLAP
(`ext/latency.h`) n'autorise `changed` que pendant `activate` ; un plug-in
actif doit seulement demander un redémarrage.

Correctif :
- `on_main_thread` : plug-in actif, `request_restart` seul ; inactif, rien
  (le prochain `activate` annonce la valeur) ;
- `activate` : nouveau champ `latency_reported` (latence que l'hôte connaît) ;
  si la latence fraîche en diffère, `changed` est appelé à ce moment-là.

Test : `crates/quack-plugin/tests/clap_host.rs`
(`latency_change_while_active_goes_through_a_restart`).

À proposer en amont (truce-audio/truce).

## Patch 2 : `params.rescan` après un chargement d'état (2026-11-12)

Fichier : `src/lib.rs`, `state_load` et `preset_load_from_location`.

Problème signalé par clap-validator (`state-reproducibility-basic`,
`-binary`, `-buffered`) : le chargement réapplique bien les valeurs, mais
sans prévenir l'hôte. CLAP demande `clap_host_params::rescan` quand les
valeurs changent hors du flux d'événements ; sinon un hôte garde à l'écran
les anciennes valeurs.

Correctif : après l'application de l'état (actif ou non), appel direct de
`rescan(CLAP_PARAM_RESCAN_VALUES)`, sur le thread principal où ces deux
fonctions sont appelées.

Test : `crates/quack-plugin/tests/clap_host.rs`
(`state_round_trip_restores_every_parameter`).
