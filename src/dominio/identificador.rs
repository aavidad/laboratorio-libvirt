// Laboratorio de máquinas virtuales con libvirt
// Copyright (C) 2026 Alberto Avidad
// SPDX-License-Identifier: GPL-3.0-or-later

use serde::{de::Error as ErrorDeserializacion, Deserialize, Deserializer, Serialize};
use std::fmt::{Display, Formatter};
use thiserror::Error;

const LONGITUD_MAXIMA: usize = 80;

/// Identificador opaco admitido en las fronteras públicas. No puede representar
/// una ruta, una opción de línea de órdenes ni una expresión del intérprete.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct Identificador(String);

impl Identificador {
    pub fn nuevo(valor: impl Into<String>) -> Result<Self, ErrorIdentificador> {
        let valor = valor.into();
        if valor.is_empty() || valor.len() > LONGITUD_MAXIMA {
            return Err(ErrorIdentificador::FormatoNoValido);
        }
        if !valor
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
            || !valor
                .as_bytes()
                .last()
                .is_some_and(u8::is_ascii_alphanumeric)
            || !valor
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(ErrorIdentificador::FormatoNoValido);
        }
        Ok(Self(valor))
    }

    pub fn como_str(&self) -> &str {
        &self.0
    }
}

impl Display for Identificador {
    fn fmt(&self, formato: &mut Formatter<'_>) -> std::fmt::Result {
        formato.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Identificador {
    fn deserialize<D>(deserializador: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let valor = String::deserialize(deserializador)?;
        Self::nuevo(valor).map_err(D::Error::custom)
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum ErrorIdentificador {
    #[error("identificador no válido")]
    FormatoNoValido,
}

#[cfg(test)]
mod pruebas {
    use super::*;

    #[test]
    fn rechaza_rutas_opciones_y_texto_ambiguo() {
        for valor in ["", "../reserva", "--ayuda", "con espacio", "á", "final-"] {
            assert_eq!(
                Identificador::nuevo(valor).unwrap_err(),
                ErrorIdentificador::FormatoNoValido
            );
        }
    }

    #[test]
    fn conserva_un_identificador_seguro() {
        let identificador = Identificador::nuevo("prueba_2026-08-02").unwrap();
        assert_eq!(identificador.como_str(), "prueba_2026-08-02");
    }

    #[test]
    fn valida_tambien_al_deserializar() {
        assert!(serde_json::from_str::<Identificador>("\"../fuera\"").is_err());
    }
}
