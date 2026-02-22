# ENOR runway selector

[![pre-commit.ci status](https://results.pre-commit.ci/badge/github/meltinglava/ENOR_Vatsim_Runway_Selector/main.svg)](https://results.pre-commit.ci/latest/github/meltinglava/ENOR_Vatsim_Runway_Selector/main)

This application is a work in progress.

It will be used to set default runways for euroscope in Norway. Do not rely on
this for any real world operation.

This is based on a lot of work by [Adrian2k](https://github.com/Adrian2k/ENOR-autorwy)

## App launcher

The application has the ability to open euroscope (and other applications).
This needs to be configured.

1. open file explorer (`windowsbutton + e` default shortcut).
2. Go to path: `%APPDATA%/meltinglava/es_runway_selector/config`. You can paste it
   in on the top just click on the unfilled area at the top showing current path.
   - If the folder does not exist just run `es_runway_selector` once. It will
     create it
3. copy file [app_launchers.toml](es_runway_selector/app_launchers.toml) to the
   folder you just opened.
4. Edit the file as you see fit. How many Euroscope instances that you want, and
   give name of the prf file you want to start each euroscope instance in.

### Any issues

Please report them on this [github issue
board](https://github.com/meltinglava/ENOR_Vatsim_Runway_Selector/issues).
But before reporting just make sure that no one has already reported it.

#### License

<sup>
Licensed under either of <a href="LICENSE-APACHE">Apache License, Version
2.0</a> or <a href="LICENSE-MIT">MIT license</a> at your option.
</sup>

<br>

<sub>
Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this crate by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.
</sub>
