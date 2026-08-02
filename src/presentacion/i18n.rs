// Laboratorio de máquinas virtuales con libvirt
// Copyright (C) 2026 Alberto Avidad
// SPDX-License-Identifier: GPL-3.0-or-later

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

const CATALOGO_CASTELLANO: &str = include_str!("../../recursos/i18n/es.json");
const MAXIMO_BYTES_CATALOGO: u64 = 1024 * 1024;

pub trait Traductor {
    fn idioma(&self) -> &str;
    fn texto<'a>(&'a self, clave: &'a str) -> &'a str;
}

/// Catálogo de mensajes de una entrada concreta. El castellano es el catálogo
/// canónico; añadir idiomas no requiere tocar el dominio ni los casos de uso.
#[derive(Debug, Clone)]
pub struct CatalogoMensajes {
    idioma: String,
    mensajes: BTreeMap<String, String>,
}

impl CatalogoMensajes {
    pub fn seleccionar(idioma_solicitado: Option<&str>) -> Result<Self> {
        Self::seleccionar_con_directorio(idioma_solicitado, None)
    }

    pub fn seleccionar_con_directorio(
        idioma_solicitado: Option<&str>,
        directorio: Option<&Path>,
    ) -> Result<Self> {
        let idioma = normalizar_idioma(idioma_solicitado.unwrap_or("es"));
        if idioma == "es" {
            return Self::desde_json("es", CATALOGO_CASTELLANO);
        }
        if idioma.bytes().all(|byte| byte.is_ascii_lowercase()) && (2..=8).contains(&idioma.len()) {
            if let Some(directorio) = directorio {
                let ruta = directorio.join(format!("{idioma}.json"));
                if ruta.try_exists()? {
                    let metadatos = fs::symlink_metadata(&ruta)?;
                    if !metadatos.file_type().is_symlink()
                        && metadatos.is_file()
                        && metadatos.len() <= MAXIMO_BYTES_CATALOGO
                    {
                        let contenido = fs::read_to_string(ruta)?;
                        return Self::desde_json_con_respaldo(&idioma, &contenido);
                    }
                }
            }
        }
        // El catálogo castellano es la alternativa segura mientras no se
        // instale una traducción válida.
        Self::desde_json("es", CATALOGO_CASTELLANO)
    }

    pub fn desde_json(idioma: &str, contenido: &str) -> Result<Self> {
        let mensajes: BTreeMap<String, String> =
            serde_json::from_str(contenido).context("el catálogo de mensajes no es JSON válido")?;
        Ok(Self {
            idioma: idioma.to_owned(),
            mensajes,
        })
    }

    fn desde_json_con_respaldo(idioma: &str, contenido: &str) -> Result<Self> {
        let mut base: BTreeMap<String, String> = serde_json::from_str(CATALOGO_CASTELLANO)
            .context("el catálogo castellano integrado no es JSON válido")?;
        let adicional: BTreeMap<String, String> = serde_json::from_str(contenido)
            .context("el catálogo de mensajes adicional no es JSON válido")?;
        base.extend(adicional);
        Ok(Self {
            idioma: idioma.to_owned(),
            mensajes: base,
        })
    }
}

impl Traductor for CatalogoMensajes {
    fn idioma(&self) -> &str {
        &self.idioma
    }

    fn texto<'a>(&'a self, clave: &'a str) -> &'a str {
        self.mensajes
            .get(clave)
            .map(String::as_str)
            .unwrap_or(clave)
    }
}

fn normalizar_idioma(valor: &str) -> String {
    valor
        .split(',')
        .next()
        .unwrap_or("es")
        .split(';')
        .next()
        .unwrap_or("es")
        .trim()
        .split('-')
        .next()
        .unwrap_or("es")
        .to_ascii_lowercase()
}

#[cfg(test)]
mod pruebas {
    use super::*;

    #[test]
    fn ofrece_ayuda_castellana_por_defecto() {
        let catalogo = CatalogoMensajes::seleccionar(None).unwrap();
        assert_eq!(catalogo.idioma(), "es");
        assert!(catalogo
            .texto("ayuda_general")
            .contains("ÓRDENES DE CONSULTA"));
    }

    #[test]
    fn acepta_negociacion_regional_y_conserva_respaldo() {
        let regional = CatalogoMensajes::seleccionar(Some("es-ES,es;q=0.9")).unwrap();
        assert_eq!(regional.idioma(), "es");
        let no_instalado = CatalogoMensajes::seleccionar(Some("fr")).unwrap();
        assert_eq!(no_instalado.idioma(), "es");
    }

    #[test]
    fn carga_un_catalogo_externo_sin_tocar_el_dominio() {
        let temporal = tempfile::tempdir().unwrap();
        fs::write(
            temporal.path().join("xx.json"),
            r#"{"ayuda_general":"Ayuda alternativa"}"#,
        )
        .unwrap();
        let catalogo =
            CatalogoMensajes::seleccionar_con_directorio(Some("xx"), Some(temporal.path()))
                .unwrap();
        assert_eq!(catalogo.idioma(), "xx");
        assert_eq!(catalogo.texto("ayuda_general"), "Ayuda alternativa");
    }
}
