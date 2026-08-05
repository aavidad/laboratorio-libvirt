// Laboratorio de máquinas virtuales con libvirt
// Copyright (C) 2026 Alberto Avidad
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::dominio::identificador::Identificador;
use crate::dominio::plantilla::Plantilla;
use serde::{Deserialize, Serialize};

pub const CICLOS_EXIGIDOS: u8 = 2;

/// Destino declarativo permitido por el inventario privado. El dominio solo
/// conoce la identidad pública y el origen autorizado, nunca nombres libvirt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DestinoPromocion {
    pub plantilla: Plantilla,
    pub id_origen: Identificador,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FasePreparacionPlantilla {
    Saneada,
    CicloUnoEnCurso,
    CicloUnoValidado,
    CicloDosEnCurso,
    CicloDosValidado,
    PromocionEnCurso,
    Promovida,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidacionCicloPlantilla {
    pub numero: u8,
    pub iniciada_en_unix_ms: u64,
    pub detenida_en_unix_ms: u64,
    pub validada_en_unix_ms: u64,
    pub identidad_ssh: ComprobacionIdentidadSsh,
    pub comprobaciones: Vec<ComprobacionCandidata>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgresoPreparacionPlantilla {
    pub id_plantilla_destino: Identificador,
    pub fase: FasePreparacionPlantilla,
    pub ciclos_validados: Vec<ValidacionCicloPlantilla>,
    #[serde(default)]
    pub ciclo_iniciado_en_unix_ms: Option<u64>,
    #[serde(default)]
    pub ciclo_detenido_en_unix_ms: Option<u64>,
    #[serde(default)]
    pub identidad_ssh_en_curso: Option<ComprobacionIdentidadSsh>,
}

/// Observación de la identidad del servidor SSH realizada mientras el huésped
/// estaba activo. Para destinos sin SSH queda marcada como no aplicable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComprobacionIdentidadSsh {
    pub aplicable: bool,
    pub correcta: bool,
    pub observada_en_unix_ms: Option<u64>,
    pub huella_sha256: Option<String>,
}

impl ComprobacionIdentidadSsh {
    pub fn no_aplicable() -> Self {
        Self {
            aplicable: false,
            correcta: false,
            observada_en_unix_ms: None,
            huella_sha256: None,
        }
    }

    pub fn validada(instante_unix_ms: u64, huella_sha256: impl Into<String>) -> Self {
        Self {
            aplicable: true,
            correcta: true,
            observada_en_unix_ms: Some(instante_unix_ms),
            huella_sha256: Some(huella_sha256.into()),
        }
    }

    pub fn es_aceptable(&self) -> bool {
        (!self.aplicable
            && !self.correcta
            && self.observada_en_unix_ms.is_none()
            && self.huella_sha256.is_none())
            || (self.aplicable
                && self.correcta
                && self.observada_en_unix_ms.is_some()
                && self
                    .huella_sha256
                    .as_deref()
                    .is_some_and(huella_sha256_ssh_canonica))
    }
}

fn huella_sha256_ssh_canonica(valor: &str) -> bool {
    valor.strip_prefix("SHA256:").is_some_and(|cuerpo| {
        cuerpo.len() == 43
            && cuerpo
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/'))
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EstadoCandidataPlantilla {
    pub apagada: bool,
    pub sin_medios_temporales: bool,
    pub red_final_conforme: bool,
    pub disco_sistema_unico: bool,
    pub identidad_independiente: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodigoComprobacionCandidata {
    CandidataApagada,
    SinMediosTemporales,
    RedFinalConforme,
    DiscoSistemaUnico,
    IdentidadIndependiente,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComprobacionCandidata {
    pub codigo: CodigoComprobacionCandidata,
    pub correcta: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticoCandidata {
    pub preparada: bool,
    pub comprobaciones: Vec<ComprobacionCandidata>,
}

pub fn diagnosticar_candidata(estado: EstadoCandidataPlantilla) -> DiagnosticoCandidata {
    let comprobaciones = vec![
        ComprobacionCandidata {
            codigo: CodigoComprobacionCandidata::CandidataApagada,
            correcta: estado.apagada,
        },
        ComprobacionCandidata {
            codigo: CodigoComprobacionCandidata::SinMediosTemporales,
            correcta: estado.sin_medios_temporales,
        },
        ComprobacionCandidata {
            codigo: CodigoComprobacionCandidata::RedFinalConforme,
            correcta: estado.red_final_conforme,
        },
        ComprobacionCandidata {
            codigo: CodigoComprobacionCandidata::DiscoSistemaUnico,
            correcta: estado.disco_sistema_unico,
        },
        ComprobacionCandidata {
            codigo: CodigoComprobacionCandidata::IdentidadIndependiente,
            correcta: estado.identidad_independiente,
        },
    ];
    DiagnosticoCandidata {
        preparada: comprobaciones.iter().all(|valor| valor.correcta),
        comprobaciones,
    }
}

#[cfg(test)]
mod pruebas {
    use super::*;

    #[test]
    fn modela_identidad_ssh_aplicable_y_no_aplicable_sin_ambiguedad() {
        assert!(ComprobacionIdentidadSsh::no_aplicable().es_aceptable());
        assert!(ComprobacionIdentidadSsh::validada(
            10,
            "SHA256:m/ejOXkI0ofKT+bW34mhHdSH63xu9c+QU7djuKirDXM"
        )
        .es_aceptable());
        assert!(!ComprobacionIdentidadSsh {
            aplicable: true,
            correcta: false,
            observada_en_unix_ms: Some(10),
            huella_sha256: Some("SHA256:m/ejOXkI0ofKT+bW34mhHdSH63xu9c+QU7djuKirDXM".to_owned(),),
        }
        .es_aceptable());
    }
}
