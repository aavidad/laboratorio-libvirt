// Laboratorio de máquinas virtuales con libvirt
// Copyright (C) 2026 Alberto Avidad
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::dominio::identificador::Identificador;
use crate::dominio::plantilla::SistemaInvitado;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CuotasResultados {
    pub maximo_artefactos: usize,
    pub maximo_bytes: u64,
    pub maximo_bytes_manifiesto: u64,
}

impl Default for CuotasResultados {
    fn default() -> Self {
        Self {
            maximo_artefactos: 5_000,
            maximo_bytes: 512 * 1024 * 1024,
            maximo_bytes_manifiesto: 2 * 1024 * 1024,
        }
    }
}

impl CuotasResultados {
    pub fn es_valida(self) -> bool {
        (1..=100_000).contains(&self.maximo_artefactos)
            && (1..=100 * 1024 * 1024 * 1024).contains(&self.maximo_bytes)
            && (1..=64 * 1024 * 1024).contains(&self.maximo_bytes_manifiesto)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EstadoReserva {
    Preparada,
    EnEjecucion,
    Detenida,
    ResultadosProtegidos,
    Fallida,
    Descartada,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultadosProtegidos {
    pub sha256_manifiesto: String,
    pub artefactos: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MotivoFallo {
    Infraestructura,
    CargaTrabajo,
    Cancelada,
    ResultadosIrrecuperables,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReciboReserva {
    pub version: u8,
    pub revision: u64,
    pub id_ejecucion: Identificador,
    pub id_plantilla: Identificador,
    pub id_instancia: Identificador,
    pub sistema: SistemaInvitado,
    pub estado: EstadoReserva,
    pub creado_en_unix_ms: u64,
    pub actualizado_en_unix_ms: u64,
    pub resultados: Option<ResultadosProtegidos>,
    pub motivo_fallo: Option<MotivoFallo>,
}

impl ReciboReserva {
    pub fn nuevo(
        id_ejecucion: Identificador,
        id_plantilla: Identificador,
        sistema: SistemaInvitado,
        instante_unix_ms: u64,
    ) -> Result<Self, ErrorReserva> {
        let id_instancia = Identificador::nuevo(format!("lab-{id_ejecucion}"))
            .map_err(|_| ErrorReserva::IdentificadorInstanciaNoValido)?;
        Ok(Self {
            version: 1,
            revision: 0,
            id_ejecucion,
            id_plantilla,
            id_instancia,
            sistema,
            estado: EstadoReserva::Preparada,
            creado_en_unix_ms: instante_unix_ms,
            actualizado_en_unix_ms: instante_unix_ms,
            resultados: None,
            motivo_fallo: None,
        })
    }

    pub fn iniciar(&mut self, instante_unix_ms: u64) -> Result<(), ErrorReserva> {
        if !matches!(
            self.estado,
            EstadoReserva::Preparada | EstadoReserva::Detenida
        ) {
            return Err(ErrorReserva::TransicionNoValida);
        }
        self.cambiar_estado(EstadoReserva::EnEjecucion, instante_unix_ms)
    }

    pub fn detener(&mut self, instante_unix_ms: u64) -> Result<(), ErrorReserva> {
        if self.estado != EstadoReserva::EnEjecucion {
            return Err(ErrorReserva::TransicionNoValida);
        }
        self.cambiar_estado(EstadoReserva::Detenida, instante_unix_ms)
    }

    pub fn proteger_resultados(
        &mut self,
        resultados: ResultadosProtegidos,
        instante_unix_ms: u64,
    ) -> Result<(), ErrorReserva> {
        if self.estado != EstadoReserva::Detenida {
            return Err(ErrorReserva::TransicionNoValida);
        }
        if resultados.sha256_manifiesto.len() != 64
            || !resultados
                .sha256_manifiesto
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(ErrorReserva::ResumenNoValido);
        }
        self.resultados = Some(resultados);
        self.cambiar_estado(EstadoReserva::ResultadosProtegidos, instante_unix_ms)
    }

    pub fn marcar_fallida(
        &mut self,
        motivo: MotivoFallo,
        instante_unix_ms: u64,
    ) -> Result<(), ErrorReserva> {
        if !matches!(
            self.estado,
            EstadoReserva::Preparada | EstadoReserva::Detenida
        ) {
            return Err(ErrorReserva::TransicionNoValida);
        }
        self.motivo_fallo = Some(motivo);
        self.cambiar_estado(EstadoReserva::Fallida, instante_unix_ms)
    }

    pub fn descartar(&mut self, instante_unix_ms: u64) -> Result<(), ErrorReserva> {
        if self.estado != EstadoReserva::ResultadosProtegidos {
            return Err(ErrorReserva::ResultadosNoProtegidos);
        }
        self.cambiar_estado(EstadoReserva::Descartada, instante_unix_ms)
    }

    pub fn descartar_fallida(&mut self, instante_unix_ms: u64) -> Result<(), ErrorReserva> {
        if self.estado != EstadoReserva::Fallida || self.motivo_fallo.is_none() {
            return Err(ErrorReserva::TransicionNoValida);
        }
        self.cambiar_estado(EstadoReserva::Descartada, instante_unix_ms)
    }

    fn cambiar_estado(
        &mut self,
        estado: EstadoReserva,
        instante_unix_ms: u64,
    ) -> Result<(), ErrorReserva> {
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or(ErrorReserva::RevisionAgotada)?;
        self.estado = estado;
        self.actualizado_en_unix_ms = instante_unix_ms;
        Ok(())
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum ErrorReserva {
    #[error("identificador de instancia no válido")]
    IdentificadorInstanciaNoValido,
    #[error("transición de reserva no válida")]
    TransicionNoValida,
    #[error("los resultados no están protegidos")]
    ResultadosNoProtegidos,
    #[error("SHA-256 de resultados no válido")]
    ResumenNoValido,
    #[error("se agotó la revisión del recibo")]
    RevisionAgotada,
}

#[cfg(test)]
mod pruebas {
    use super::*;

    fn recibo() -> ReciboReserva {
        ReciboReserva::nuevo(
            Identificador::nuevo("ejecucion-001").unwrap(),
            Identificador::nuevo("windows-analisis").unwrap(),
            SistemaInvitado::Windows,
            1,
        )
        .unwrap()
    }

    #[test]
    fn exige_resultados_protegidos_para_el_descarte_normal() {
        let mut valor = recibo();
        valor.iniciar(2).unwrap();
        valor.detener(3).unwrap();
        assert_eq!(
            valor.descartar(4).unwrap_err(),
            ErrorReserva::ResultadosNoProtegidos
        );
        valor
            .proteger_resultados(
                ResultadosProtegidos {
                    sha256_manifiesto: "a".repeat(64),
                    artefactos: 2,
                    bytes: 32,
                },
                4,
            )
            .unwrap();
        valor.descartar(5).unwrap();
        assert_eq!(valor.estado, EstadoReserva::Descartada);
        assert_eq!(valor.revision, 4);
    }
}
