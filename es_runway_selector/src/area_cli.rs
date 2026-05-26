//! `es_runway_selector area …` subcommands. Wraps the
//! [`runway_selector_areas`] crate with the CLI surface and resolves the
//! install directory from [`crate::config::es_runway_selector_project_dir`].

use std::path::{Path, PathBuf};

use clap::Subcommand;
use runway_selector_areas::{
    fetch_combined_registry, install_area, list_installed_areas, remove_area,
};
use runway_selector_core::area_config::{TopLevelConfig, load_with_local_override};
use tracing::info;

use crate::{config::es_runway_selector_project_dir, error::ApplicationResult};

#[derive(Debug, Subcommand)]
pub enum AreaCommand {
    /// List areas installed locally.
    List,
    /// List areas available from the configured registries.
    Available,
    /// Install an area by name from the registry.
    Install { name: String },
    /// Remove an installed area by name.
    Remove { name: String },
    /// Inspect profiles within an installed area.
    Profile {
        #[command(subcommand)]
        cmd: ProfileCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum ProfileCommand {
    /// List profiles across every installed area.
    List,
    /// Print the resolved (base + .local override) contents of one profile.
    Show { area: String, profile: String },
}

pub async fn run_area_command(cmd: AreaCommand) -> ApplicationResult<()> {
    let top_level = load_top_level_config()?;
    let install_dir = resolved_install_dir(&top_level);

    match cmd {
        AreaCommand::List => print_installed(&install_dir)?,
        AreaCommand::Available => print_available(&top_level).await?,
        AreaCommand::Install { name } => do_install(&top_level, &install_dir, &name).await?,
        AreaCommand::Remove { name } => do_remove(&install_dir, &name)?,
        AreaCommand::Profile { cmd } => run_profile_command(cmd, &install_dir)?,
    }
    Ok(())
}

fn run_profile_command(cmd: ProfileCommand, install_dir: &Path) -> ApplicationResult<()> {
    match cmd {
        ProfileCommand::List => print_profiles(install_dir),
        ProfileCommand::Show { area, profile } => print_profile(install_dir, &area, &profile),
    }
}

fn print_profiles(install_dir: &Path) -> ApplicationResult<()> {
    let areas = crate::wizard::list_areas_with_profiles(install_dir)?;
    if areas.is_empty() {
        println!("No areas installed.");
        return Ok(());
    }
    for (manifest, profiles) in areas {
        if profiles.is_empty() {
            println!("{name}: (no profiles)", name = manifest.name);
            continue;
        }
        println!("{name}:", name = manifest.name);
        for p in profiles {
            println!("  {n:20} {d}", n = p.name, d = p.display_name);
        }
    }
    Ok(())
}

fn print_profile(install_dir: &Path, area: &str, profile: &str) -> ApplicationResult<()> {
    match crate::wizard::load_profile_in_area(install_dir, area, profile)? {
        Some(p) => {
            println!("name        : {}", p.name);
            println!("display_name: {}", p.display_name);
            println!("prf_files   : {:?}", p.prf_files);
            println!("default_apps: {:?}", p.default_apps);
        }
        None => println!("Profile {area}/{profile} not found"),
    }
    Ok(())
}

pub fn load_top_level_config() -> ApplicationResult<TopLevelConfig> {
    let path = es_runway_selector_project_dir()
        .config_dir()
        .join("config.toml");
    Ok(load_with_local_override::<TopLevelConfig>(&path)?)
}

pub fn resolved_install_dir(cfg: &TopLevelConfig) -> PathBuf {
    cfg.areas_install_dir
        .clone()
        .unwrap_or_else(|| es_runway_selector_project_dir().data_dir().join("areas"))
}

fn print_installed(install_dir: &Path) -> ApplicationResult<()> {
    let installed = list_installed_areas(install_dir).map_err(map_area_err)?;
    if installed.is_empty() {
        println!("No areas installed in {}", install_dir.display());
        return Ok(());
    }
    println!("Installed areas in {}:", install_dir.display());
    for (path, manifest) in installed {
        println!(
            "  {name:20} {version}  ({display_name})  [{path}]",
            name = manifest.name,
            version = manifest.version,
            display_name = manifest.display_name,
            path = path.display(),
        );
    }
    Ok(())
}

async fn print_available(cfg: &TopLevelConfig) -> ApplicationResult<()> {
    let registry = fetch_combined_registry(cfg).await.map_err(map_area_err)?;
    if registry.areas.is_empty() {
        println!("No areas in registry");
        return Ok(());
    }
    println!("Areas available in registry:");
    for area in registry.areas {
        println!(
            "  {name:20} {version}  {display_name}\n      {desc}",
            name = area.name,
            version = area.version,
            display_name = area.display_name,
            desc = area.description,
        );
    }
    Ok(())
}

async fn do_install(cfg: &TopLevelConfig, install_dir: &Path, name: &str) -> ApplicationResult<()> {
    let registry = fetch_combined_registry(cfg).await.map_err(map_area_err)?;
    let entry = registry
        .areas
        .into_iter()
        .find(|a| a.name == name)
        .ok_or_else(|| runway_selector_areas::AreaRegistryError::UnknownArea {
            name: name.to_string(),
        })
        .map_err(map_area_err)?;

    let installed = install_area(&entry, install_dir)
        .await
        .map_err(map_area_err)?;
    info!(name = %entry.name, version = %entry.version, path = %installed.display(), "Area installed");
    println!(
        "Installed {} v{} to {}",
        entry.name,
        entry.version,
        installed.display()
    );
    Ok(())
}

fn do_remove(install_dir: &Path, name: &str) -> ApplicationResult<()> {
    remove_area(install_dir, name).map_err(map_area_err)?;
    println!("Removed area {name} (if it was installed)");
    Ok(())
}

fn map_area_err(e: runway_selector_areas::AreaRegistryError) -> crate::error::ApplicationError {
    crate::error::ApplicationError::Areas(e)
}
