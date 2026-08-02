// Laboratorio de máquinas virtuales con libvirt
// Copyright (C) 2026 Alberto Avidad
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::aplicacion::ordenes::Orden;
use crate::dominio::identificador::Identificador;
use crate::dominio::reserva::MotivoFallo;
use anyhow::{bail, Context, Result};
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

#[derive(Debug)]
pub enum AccionCli {
    Ayuda {
        idioma: Option<String>,
    },
    Version,
    Ejecutar {
        configuracion: PathBuf,
        idioma: Option<String>,
        orden: Orden,
    },
    ServirApi {
        configuracion: PathBuf,
        idioma: Option<String>,
    },
}

pub fn analizar(argumentos: Vec<OsString>) -> Result<AccionCli> {
    if argumentos.is_empty()
        || argumentos.as_slice() == [OsString::from("--ayuda")]
        || argumentos.as_slice() == [OsString::from("ayuda")]
    {
        return Ok(AccionCli::Ayuda { idioma: None });
    }
    if argumentos.len() == 3
        && argumentos[0] == "--idioma"
        && (argumentos[2] == "--ayuda" || argumentos[2] == "ayuda")
    {
        return Ok(AccionCli::Ayuda {
            idioma: argumentos[1].to_str().map(str::to_owned),
        });
    }
    if argumentos.as_slice() == [OsString::from("--version")] {
        return Ok(AccionCli::Version);
    }

    let mut indice = 0_usize;
    let mut idioma = None;
    if argumentos.get(indice).map(OsString::as_os_str) == Some(OsStr::new("--idioma")) {
        idioma = Some(
            argumentos
                .get(indice + 1)
                .and_then(|valor| valor.to_str())
                .context("falta un código de idioma válido")?
                .to_owned(),
        );
        indice += 2;
    }
    if argumentos.get(indice).map(OsString::as_os_str) != Some(OsStr::new("--configuracion")) {
        bail!("falta --configuracion <ruta>; use --ayuda");
    }
    let configuracion = PathBuf::from(
        argumentos
            .get(indice + 1)
            .context("falta la ruta de configuración")?,
    );
    indice += 2;
    let orden = argumentos
        .get(indice)
        .and_then(|valor| valor.to_str())
        .context("falta una orden válida")?;
    let resto = &argumentos[indice + 1..];
    if orden == "servir-api" {
        if !resto.is_empty() {
            bail!("servir-api no admite argumentos adicionales");
        }
        return Ok(AccionCli::ServirApi {
            configuracion,
            idioma,
        });
    }
    Ok(AccionCli::Ejecutar {
        configuracion,
        idioma,
        orden: analizar_orden(orden, resto)?,
    })
}

fn analizar_orden(nombre: &str, argumentos: &[OsString]) -> Result<Orden> {
    match nombre {
        "plantillas" => {
            exigir_cantidad(argumentos, 0, nombre)?;
            Ok(Orden::ListarPlantillas)
        }
        "reservas" => {
            exigir_cantidad(argumentos, 0, nombre)?;
            Ok(Orden::ListarReservas)
        }
        "inspeccionar" => {
            exigir_cantidad(argumentos, 1, nombre)?;
            Ok(Orden::Inspeccionar {
                id_plantilla: identificador(&argumentos[0], "id de plantilla")?,
            })
        }
        "estado" => {
            exigir_cantidad(argumentos, 1, nombre)?;
            Ok(Orden::Estado {
                id_ejecucion: identificador(&argumentos[0], "id de ejecución")?,
            })
        }
        "acceso" => {
            exigir_cantidad(argumentos, 1, nombre)?;
            Ok(Orden::Acceso {
                id_ejecucion: identificador(&argumentos[0], "id de ejecución")?,
            })
        }
        "preparar" => {
            exigir_cantidad(argumentos, 4, nombre)?;
            exigir_bandera(&argumentos[2], "--confirmar")?;
            Ok(Orden::Preparar {
                id_plantilla: identificador(&argumentos[0], "id de plantilla")?,
                id_ejecucion: identificador(&argumentos[1], "id de ejecución")?,
                confirmacion: identificador(&argumentos[3], "confirmación")?,
            })
        }
        "iniciar" | "detener" | "reconciliar" | "proteger-resultados" | "descartar" => {
            exigir_cantidad(argumentos, 3, nombre)?;
            exigir_bandera(&argumentos[1], "--confirmar")?;
            let id_ejecucion = identificador(&argumentos[0], "id de ejecución")?;
            let confirmacion = identificador(&argumentos[2], "confirmación")?;
            Ok(match nombre {
                "iniciar" => Orden::Iniciar {
                    id_ejecucion,
                    confirmacion,
                },
                "detener" => Orden::Detener {
                    id_ejecucion,
                    confirmacion,
                },
                "reconciliar" => Orden::Reconciliar {
                    id_ejecucion,
                    confirmacion,
                },
                "proteger-resultados" => Orden::ProtegerResultados {
                    id_ejecucion,
                    confirmacion,
                },
                "descartar" => Orden::Descartar {
                    id_ejecucion,
                    confirmacion,
                },
                _ => unreachable!(),
            })
        }
        "fallar" => {
            exigir_cantidad(argumentos, 4, nombre)?;
            exigir_bandera(&argumentos[2], "--confirmar")?;
            Ok(Orden::MarcarFallida {
                id_ejecucion: identificador(&argumentos[0], "id de ejecución")?,
                motivo: motivo(&argumentos[1])?,
                confirmacion: identificador(&argumentos[3], "confirmación")?,
            })
        }
        "descartar-fallida" => {
            exigir_cantidad(argumentos, 4, nombre)?;
            exigir_bandera(&argumentos[1], "--aceptar-perdida-resultados")?;
            exigir_bandera(&argumentos[2], "--confirmar")?;
            Ok(Orden::DescartarFallida {
                id_ejecucion: identificador(&argumentos[0], "id de ejecución")?,
                confirmacion: identificador(&argumentos[3], "confirmación")?,
                acepta_perdida_resultados: true,
            })
        }
        _ => bail!("orden no reconocida; use --ayuda"),
    }
}

fn exigir_cantidad(argumentos: &[OsString], esperada: usize, orden: &str) -> Result<()> {
    if argumentos.len() != esperada {
        bail!("cantidad de argumentos no válida para {orden}; use --ayuda");
    }
    Ok(())
}

fn exigir_bandera(valor: &OsString, esperada: &str) -> Result<()> {
    if valor != esperada {
        bail!("falta {esperada} en la posición esperada");
    }
    Ok(())
}

fn identificador(valor: &OsString, nombre: &str) -> Result<Identificador> {
    let valor = valor
        .to_str()
        .with_context(|| format!("{nombre} no es UTF-8"))?;
    Identificador::nuevo(valor).with_context(|| format!("{nombre} no válido"))
}

fn motivo(valor: &OsString) -> Result<MotivoFallo> {
    match valor.to_str() {
        Some("infraestructura") => Ok(MotivoFallo::Infraestructura),
        Some("carga_trabajo") => Ok(MotivoFallo::CargaTrabajo),
        Some("cancelada") => Ok(MotivoFallo::Cancelada),
        Some("resultados_irrecuperables") => Ok(MotivoFallo::ResultadosIrrecuperables),
        _ => bail!("motivo de fallo no permitido"),
    }
}

#[cfg(test)]
mod pruebas {
    use super::*;

    #[test]
    fn ofrece_ayuda_sin_configuracion() {
        assert!(matches!(
            analizar(Vec::new()).unwrap(),
            AccionCli::Ayuda { .. }
        ));
    }

    #[test]
    fn no_acepta_argumentos_adicionales() {
        let argumentos = [
            "--configuracion",
            "/tmp/config.json",
            "estado",
            "ejecucion-1",
            "extra",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        assert!(analizar(argumentos).is_err());
    }

    #[test]
    fn conserva_la_confirmacion_para_el_caso_de_uso() {
        let argumentos = [
            "--configuracion",
            "/tmp/config.json",
            "iniciar",
            "ejecucion-1",
            "--confirmar",
            "ejecucion-2",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        let AccionCli::Ejecutar {
            orden:
                Orden::Iniciar {
                    id_ejecucion,
                    confirmacion,
                },
            ..
        } = analizar(argumentos).unwrap()
        else {
            panic!("orden inesperada");
        };
        assert_ne!(id_ejecucion, confirmacion);
    }
}
