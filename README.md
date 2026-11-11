# Quackapella — DuckDSP

**Quackapella** est un plugin de manipulation vocale en temps réel écrit
en Rust, édité sous le nom **DuckDSP**. Il réunit, dans un seul plugin
pensé pour le live (latence d'environ 10 ms), la correction de hauteur
(autotune), la manipulation façon Manipulator (pitch et formants séparés,
freeze, glitch) et la synthèse façon VocalSynth (harmonies jouées au MIDI,
vocodeur, talkbox).

Exigence centrale : un **mode Naturel** qui corrige ou transpose la voix,
même fortement, sans son artificiel (formants préservés, consonnes et
souffle intacts, correction humanisée).

> Statut : étape 1 de la feuille de route (socle). Le plugin est un
> passthrough avec entrée MIDI, paramètres de base et latence déclarée ;
> le traitement de la voix arrive aux étapes suivantes. Formats : **AU v2
> (Logic Pro, macOS)** en priorité, VST3 et CLAP aussi compilés.

![Éditeur, mode Natural](docs/design/editor-natural.png)

Interface dans la direction graphique DuckDSP : thème et marque de Sonder,
boutons rotatifs de Knock avec la valeur écrite dedans. Captures régénérées
par `QUACK_WRITE_CAPTURES=1 cargo test -p quack-ui --test design`.

## Compiler et installer

Prérequis : Rust stable (`rust-toolchain.toml`), `cargo-truce` 6.3.0,
macOS pour l'AU. Le framework est [Truce](https://crates.io/crates/truce)
6.3.0, le même que Sonder, avec les mêmes patchs vendorés
(`vendor/truce-au`, `vendor/truce-egui`).

```sh
cargo install cargo-truce --version =6.3.0 --locked
cargo test --workspace --features quack-plugin/rt-paranoid   # tests (Linux ou macOS)
scripts/bundle.sh                    # bundles dans target/bundles/, zips dans dist/
scripts/bundle.sh --install          # idem + installation utilisateur
auval -strict -v aumf Qkpl DKDS      # validation Apple de l'AU
scripts/validate-macos.sh            # tests + AU + auval + VST3/pluginval, journaux dans validation-logs/
```

La CI GitHub Actions (`.github/workflows/ci.yml`) vérifie fmt, clippy et
les tests sous Linux, puis construit sur macOS les bundles AU v2, VST3 et
CLAP, passe `auval` et `pluginval`, et publie les zips en artefact
`quackapella-macos`.

Dans Logic, l'AU s'enregistre comme effet MIDI-contrôlé (`aumf`) : on
l'insère sur la piste audio de la voix, puis on choisit cette piste comme
sortie MIDI d'une piste instrument (sélecteur de side-chain MIDI du
plugin).

## Structure du dépôt

| Dossier | Rôle |
|---|---|
| `crates/quack-dsp` | Moteur vocal temps réel : paramètres, chaîne, latence, MIDI, modes, interfaces des étapes à venir (sans framework, sans allocation après `prepare`) |
| `crates/quack-ui` | Éditeur egui dans la direction graphique DuckDSP (thème et marque repris de Sonder) |
| `crates/quack-plugin` | Intégration Truce (AU v2 / VST3 / CLAP), sans DSP |
| `vendor/truce-*` | Patchs de Truce partagés avec Sonder (voir `PATCHES.md`) |
| `scripts/` | Bundle, validation macOS |
| `docs/` | Architecture, feuille de route, décisions (ADR) |

Pour aller plus loin : [`docs/architecture.md`](docs/architecture.md) et
[`docs/decisions/`](docs/decisions/).

## Licence

Tous droits réservés (`UNLICENSED`) tant que la licence n'est pas choisie.

## Crédits

- [Inter](https://github.com/rsms/inter) 4.1, police des libellés,
  © The Inter Project Authors, SIL Open Font License 1.1
  (`crates/quack-ui/assets/Inter-OFL.txt`).
- JetBrains Mono, police des valeurs, fournie par `truce-font` (SIL OFL 1.1).
