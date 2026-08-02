// Laboratorio de máquinas virtuales con libvirt
// Copyright (C) 2026 Alberto Avidad
// SPDX-License-Identifier: GPL-3.0-or-later

use laboratorio_libvirt::adaptadores::configuracion_json::ConfiguracionLocal;
use laboratorio_libvirt::adaptadores::entrada_cli::{analizar, AccionCli};
use laboratorio_libvirt::adaptadores::entrada_http;
use laboratorio_libvirt::composicion;
use laboratorio_libvirt::presentacion::i18n::{CatalogoMensajes, Traductor};
use std::ffi::OsString;
use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    let argumentos = std::env::args_os().skip(1).collect::<Vec<OsString>>();
    match ejecutar(argumentos).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let catalogo = CatalogoMensajes::seleccionar(Some("es"))
                .expect("el catálogo castellano integrado debe ser válido");
            eprintln!("{}: {error:#}", catalogo.texto("error_prefijo"));
            ExitCode::FAILURE
        }
    }
}

async fn ejecutar(argumentos: Vec<OsString>) -> anyhow::Result<()> {
    match analizar(argumentos)? {
        AccionCli::Ayuda { idioma } => {
            let catalogo = CatalogoMensajes::seleccionar(idioma.as_deref())?;
            println!("{}", catalogo.texto("ayuda_general"));
        }
        AccionCli::Version => println!("laboratorio-libvirt {}", env!("CARGO_PKG_VERSION")),
        AccionCli::Ejecutar {
            configuracion,
            idioma,
            orden,
        } => {
            let configuracion = ConfiguracionLocal::cargar(&configuracion)?;
            let _catalogo = CatalogoMensajes::seleccionar_con_directorio(
                idioma
                    .as_deref()
                    .or(Some(&configuracion.idioma_predeterminado)),
                configuracion.directorio_idiomas.as_deref(),
            )?;
            let respuesta = composicion::ejecutar(configuracion, orden)?;
            serde_json::to_writer_pretty(std::io::stdout().lock(), &respuesta)?;
            println!();
        }
        AccionCli::ServirApi {
            configuracion,
            idioma,
        } => {
            let mut configuracion = ConfiguracionLocal::cargar(&configuracion)?;
            if let Some(idioma) = idioma {
                configuracion.idioma_predeterminado = idioma;
            }
            entrada_http::servir(configuracion).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod pruebas {
    use super::*;

    #[tokio::test]
    async fn muestra_ayuda_sin_configuracion() {
        ejecutar(vec![OsString::from("--ayuda")]).await.unwrap();
    }
}
