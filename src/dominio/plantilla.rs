// Laboratorio de máquinas virtuales con libvirt
// Copyright (C) 2026 Alberto Avidad
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::dominio::identificador::Identificador;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::net::IpAddr;

/// Familia del sistema invitado. El dominio solo la publica como capacidad;
/// no contiene ramas que ejecuten órdenes específicas de Windows o Linux.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SistemaInvitado {
    Windows,
    Linux,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PoliticaRed {
    SinRed,
    Aislada,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocoloAcceso {
    Ssh,
    WinrmHttps,
    AgenteHttps,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanalAcceso {
    pub protocolo: ProtocoloAcceso,
    pub puerto: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PuntoAcceso {
    pub protocolo: ProtocoloAcceso,
    pub direccion: IpAddr,
    pub puerto: u16,
    pub identidad_servidor: Option<IdentidadServidor>,
}

/// Identidad pública obtenida fuera de banda por el proveedor de máquinas.
/// Nunca procede de la red que después se autenticará.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IdentidadServidor {
    pub algoritmo: Identificador,
    pub clave_publica: String,
    pub huella_sha256: String,
}

/// Descripción tecnológica neutra que pueden consultar los consumidores.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plantilla {
    pub id: Identificador,
    pub sistema: SistemaInvitado,
    pub politica_red: PoliticaRed,
    pub canal_acceso: Option<CanalAcceso>,
    pub capacidades: BTreeSet<Identificador>,
}

/// Observaciones del adaptador de hipervisor expresadas sin nombres, rutas ni
/// conceptos propios de libvirt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EstadoPlantilla {
    pub apagada: bool,
    pub sin_estado_guardado: bool,
    pub identidad_coincide: bool,
    pub discos_sistema: usize,
    pub formato_incremental_compatible: bool,
    pub origen_registrado: bool,
    pub red_conforme: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodigoComprobacion {
    PlantillaApagada,
    SinEstadoGuardado,
    IdentidadRegistrada,
    DiscoSistemaUnico,
    FormatoIncrementalCompatible,
    OrigenRegistrado,
    RedConforme,
}

impl CodigoComprobacion {
    pub fn codigo(self) -> &'static str {
        match self {
            Self::PlantillaApagada => "plantilla_apagada",
            Self::SinEstadoGuardado => "sin_estado_guardado",
            Self::IdentidadRegistrada => "identidad_registrada",
            Self::DiscoSistemaUnico => "disco_sistema_unico",
            Self::FormatoIncrementalCompatible => "formato_incremental_compatible",
            Self::OrigenRegistrado => "origen_registrado",
            Self::RedConforme => "red_conforme",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComprobacionPlantilla {
    pub codigo: CodigoComprobacion,
    pub correcta: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticoPlantilla {
    pub preparada: bool,
    pub comprobaciones: Vec<ComprobacionPlantilla>,
}

pub fn diagnosticar(estado: &EstadoPlantilla) -> DiagnosticoPlantilla {
    let comprobaciones = vec![
        ComprobacionPlantilla {
            codigo: CodigoComprobacion::PlantillaApagada,
            correcta: estado.apagada,
        },
        ComprobacionPlantilla {
            codigo: CodigoComprobacion::SinEstadoGuardado,
            correcta: estado.sin_estado_guardado,
        },
        ComprobacionPlantilla {
            codigo: CodigoComprobacion::IdentidadRegistrada,
            correcta: estado.identidad_coincide,
        },
        ComprobacionPlantilla {
            codigo: CodigoComprobacion::DiscoSistemaUnico,
            correcta: estado.discos_sistema == 1,
        },
        ComprobacionPlantilla {
            codigo: CodigoComprobacion::FormatoIncrementalCompatible,
            correcta: estado.formato_incremental_compatible,
        },
        ComprobacionPlantilla {
            codigo: CodigoComprobacion::OrigenRegistrado,
            correcta: estado.origen_registrado,
        },
        ComprobacionPlantilla {
            codigo: CodigoComprobacion::RedConforme,
            correcta: estado.red_conforme,
        },
    ];
    DiagnosticoPlantilla {
        preparada: comprobaciones.iter().all(|valor| valor.correcta),
        comprobaciones,
    }
}

#[cfg(test)]
mod pruebas {
    use super::*;

    #[test]
    fn exige_todas_las_invariantes_de_la_plantilla() {
        let estado = EstadoPlantilla {
            apagada: true,
            sin_estado_guardado: true,
            identidad_coincide: true,
            discos_sistema: 2,
            formato_incremental_compatible: true,
            origen_registrado: true,
            red_conforme: true,
        };
        let diagnostico = diagnosticar(&estado);
        assert!(!diagnostico.preparada);
        assert_eq!(
            diagnostico
                .comprobaciones
                .iter()
                .find(|valor| valor.codigo == CodigoComprobacion::DiscoSistemaUnico)
                .map(|valor| valor.correcta),
            Some(false)
        );
    }
}
