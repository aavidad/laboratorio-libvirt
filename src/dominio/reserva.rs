// Laboratorio de máquinas virtuales con libvirt
// Copyright (C) 2026 Alberto Avidad
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::dominio::identificador::Identificador;
use crate::dominio::plantilla::SistemaInvitado;
use crate::dominio::preparacion_plantilla::{
    ComprobacionIdentidadSsh, DiagnosticoCandidata, FasePreparacionPlantilla,
    ProgresoPreparacionPlantilla, ValidacionCicloPlantilla,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
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
    Promovida,
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
    #[serde(default)]
    pub preparacion_plantilla: Option<ProgresoPreparacionPlantilla>,
}

impl ReciboReserva {
    pub fn nuevo(
        id_ejecucion: Identificador,
        id_plantilla: Identificador,
        sistema: SistemaInvitado,
        instante_unix_ms: u64,
    ) -> Result<Self, ErrorReserva> {
        let nombre_directo = format!("lab-{id_ejecucion}");
        let id_instancia = if nombre_directo.len() <= 64 {
            Identificador::nuevo(nombre_directo)
        } else {
            let huella = Sha256::digest(id_ejecucion.como_str().as_bytes());
            let mut sufijo = String::with_capacity(60);
            for byte in huella.iter().take(30) {
                write!(&mut sufijo, "{byte:02x}")
                    .map_err(|_| ErrorReserva::IdentificadorInstanciaNoValido)?;
            }
            Identificador::nuevo(format!("lab-{sufijo}"))
        }
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
            preparacion_plantilla: None,
        })
    }

    pub fn iniciar(&mut self, instante_unix_ms: u64) -> Result<(), ErrorReserva> {
        if self.preparacion_plantilla.is_some() {
            return Err(ErrorReserva::UsarCicloPreparacion);
        }
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
        if let Some(progreso) = &mut self.preparacion_plantilla {
            if matches!(
                progreso.fase,
                FasePreparacionPlantilla::CicloUnoEnCurso
                    | FasePreparacionPlantilla::CicloDosEnCurso
            ) {
                if !progreso
                    .identidad_ssh_en_curso
                    .as_ref()
                    .is_some_and(|comprobacion| comprobacion.es_aceptable())
                {
                    return Err(ErrorReserva::IdentidadSshCicloNoValidada);
                }
                progreso.ciclo_detenido_en_unix_ms = Some(instante_unix_ms);
            }
        }
        self.cambiar_estado(EstadoReserva::Detenida, instante_unix_ms)
    }

    pub fn proteger_resultados(
        &mut self,
        resultados: ResultadosProtegidos,
        instante_unix_ms: u64,
    ) -> Result<(), ErrorReserva> {
        if self.estado != EstadoReserva::Detenida || self.preparacion_plantilla.is_some() {
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
        ) || matches!(
            self.preparacion_plantilla.as_ref().map(|valor| valor.fase),
            Some(FasePreparacionPlantilla::PromocionEnCurso | FasePreparacionPlantilla::Promovida)
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

    pub fn registrar_saneamiento(
        &mut self,
        id_plantilla_destino: Identificador,
        instante_unix_ms: u64,
    ) -> Result<(), ErrorReserva> {
        if self.estado != EstadoReserva::Detenida {
            return Err(ErrorReserva::TransicionNoValida);
        }
        match &self.preparacion_plantilla {
            Some(progreso) if progreso.id_plantilla_destino == id_plantilla_destino => {
                return Ok(())
            }
            Some(_) => return Err(ErrorReserva::DestinoPromocionNoCoincide),
            None => {}
        }
        self.preparacion_plantilla = Some(ProgresoPreparacionPlantilla {
            id_plantilla_destino,
            fase: FasePreparacionPlantilla::Saneada,
            ciclos_validados: Vec::new(),
            ciclo_iniciado_en_unix_ms: None,
            ciclo_detenido_en_unix_ms: None,
            identidad_ssh_en_curso: None,
        });
        self.incrementar_revision(instante_unix_ms)
    }

    pub fn iniciar_ciclo_plantilla(&mut self, instante_unix_ms: u64) -> Result<u8, ErrorReserva> {
        if self.estado != EstadoReserva::Detenida {
            return Err(ErrorReserva::TransicionNoValida);
        }
        let progreso = self
            .preparacion_plantilla
            .as_mut()
            .ok_or(ErrorReserva::PreparacionNoIniciada)?;
        let numero = match progreso.fase {
            FasePreparacionPlantilla::Saneada => {
                progreso.fase = FasePreparacionPlantilla::CicloUnoEnCurso;
                1
            }
            FasePreparacionPlantilla::CicloUnoValidado => {
                progreso.fase = FasePreparacionPlantilla::CicloDosEnCurso;
                2
            }
            _ => return Err(ErrorReserva::TransicionNoValida),
        };
        progreso.ciclo_iniciado_en_unix_ms = Some(instante_unix_ms);
        progreso.ciclo_detenido_en_unix_ms = None;
        progreso.identidad_ssh_en_curso = None;
        self.estado = EstadoReserva::EnEjecucion;
        self.incrementar_revision(instante_unix_ms)?;
        Ok(numero)
    }

    pub fn registrar_identidad_ssh_ciclo(
        &mut self,
        comprobacion: ComprobacionIdentidadSsh,
        instante_unix_ms: u64,
    ) -> Result<(), ErrorReserva> {
        if self.estado != EstadoReserva::EnEjecucion || !comprobacion.es_aceptable() {
            return Err(ErrorReserva::IdentidadSshCicloNoValidada);
        }
        let progreso = self
            .preparacion_plantilla
            .as_mut()
            .ok_or(ErrorReserva::PreparacionNoIniciada)?;
        if !matches!(
            progreso.fase,
            FasePreparacionPlantilla::CicloUnoEnCurso | FasePreparacionPlantilla::CicloDosEnCurso
        ) {
            return Err(ErrorReserva::TransicionNoValida);
        }
        match progreso.identidad_ssh_en_curso.as_ref() {
            Some(existente) if existente == &comprobacion => return Ok(()),
            Some(_) => return Err(ErrorReserva::IdentidadSshCicloNoValidada),
            None => progreso.identidad_ssh_en_curso = Some(comprobacion),
        }
        self.incrementar_revision(instante_unix_ms)
    }

    pub fn registrar_validacion_ciclo(
        &mut self,
        diagnostico: DiagnosticoCandidata,
        instante_unix_ms: u64,
    ) -> Result<u8, ErrorReserva> {
        if self.estado != EstadoReserva::Detenida || !diagnostico.preparada {
            return Err(ErrorReserva::CandidataNoValida);
        }
        let progreso = self
            .preparacion_plantilla
            .as_mut()
            .ok_or(ErrorReserva::PreparacionNoIniciada)?;
        let iniciada_en_unix_ms = progreso
            .ciclo_iniciado_en_unix_ms
            .ok_or(ErrorReserva::CicloIncompleto)?;
        let detenida_en_unix_ms = progreso
            .ciclo_detenido_en_unix_ms
            .ok_or(ErrorReserva::CicloIncompleto)?;
        let identidad_ssh = progreso
            .identidad_ssh_en_curso
            .take()
            .filter(|comprobacion| comprobacion.es_aceptable())
            .ok_or(ErrorReserva::IdentidadSshCicloNoValidada)?;
        let numero = match progreso.fase {
            FasePreparacionPlantilla::CicloUnoEnCurso => {
                progreso.fase = FasePreparacionPlantilla::CicloUnoValidado;
                1
            }
            FasePreparacionPlantilla::CicloDosEnCurso => {
                progreso.fase = FasePreparacionPlantilla::CicloDosValidado;
                2
            }
            _ => return Err(ErrorReserva::TransicionNoValida),
        };
        progreso.ciclo_iniciado_en_unix_ms = None;
        progreso.ciclo_detenido_en_unix_ms = None;
        progreso.ciclos_validados.push(ValidacionCicloPlantilla {
            numero,
            iniciada_en_unix_ms,
            detenida_en_unix_ms,
            validada_en_unix_ms: instante_unix_ms,
            identidad_ssh,
            comprobaciones: diagnostico.comprobaciones,
        });
        self.incrementar_revision(instante_unix_ms)?;
        Ok(numero)
    }

    pub fn marcar_promovida(&mut self, instante_unix_ms: u64) -> Result<(), ErrorReserva> {
        if self.estado != EstadoReserva::Detenida {
            return Err(ErrorReserva::TransicionNoValida);
        }
        let progreso = self
            .preparacion_plantilla
            .as_mut()
            .ok_or(ErrorReserva::PreparacionNoIniciada)?;
        if progreso.fase != FasePreparacionPlantilla::PromocionEnCurso
            || progreso.ciclos_validados.len() != 2
            || !identidades_ssh_coherentes(&progreso.ciclos_validados)
        {
            return Err(ErrorReserva::CiclosInsuficientes);
        }
        progreso.fase = FasePreparacionPlantilla::Promovida;
        self.estado = EstadoReserva::Promovida;
        self.incrementar_revision(instante_unix_ms)
    }

    pub fn iniciar_promocion(&mut self, instante_unix_ms: u64) -> Result<(), ErrorReserva> {
        if self.estado != EstadoReserva::Detenida {
            return Err(ErrorReserva::TransicionNoValida);
        }
        let progreso = self
            .preparacion_plantilla
            .as_mut()
            .ok_or(ErrorReserva::PreparacionNoIniciada)?;
        if progreso.fase != FasePreparacionPlantilla::CicloDosValidado
            || progreso.ciclos_validados.len() != 2
            || !identidades_ssh_coherentes(&progreso.ciclos_validados)
        {
            return Err(ErrorReserva::CiclosInsuficientes);
        }
        progreso.fase = FasePreparacionPlantilla::PromocionEnCurso;
        self.incrementar_revision(instante_unix_ms)
    }

    fn incrementar_revision(&mut self, instante_unix_ms: u64) -> Result<(), ErrorReserva> {
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or(ErrorReserva::RevisionAgotada)?;
        self.actualizado_en_unix_ms = instante_unix_ms;
        Ok(())
    }

    fn cambiar_estado(
        &mut self,
        estado: EstadoReserva,
        instante_unix_ms: u64,
    ) -> Result<(), ErrorReserva> {
        self.incrementar_revision(instante_unix_ms)?;
        self.estado = estado;
        Ok(())
    }
}

fn identidades_ssh_coherentes(ciclos: &[ValidacionCicloPlantilla]) -> bool {
    let Some(primero) = ciclos.first().map(|ciclo| &ciclo.identidad_ssh) else {
        return false;
    };
    primero.es_aceptable()
        && ciclos.iter().all(|ciclo| {
            ciclo.identidad_ssh.es_aceptable()
                && ciclo.identidad_ssh.aplicable == primero.aplicable
                && ciclo.identidad_ssh.huella_sha256 == primero.huella_sha256
        })
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
    #[error("use el ciclo tipado de preparación de plantilla")]
    UsarCicloPreparacion,
    #[error("la preparación de plantilla no se ha iniciado")]
    PreparacionNoIniciada,
    #[error("el destino de promoción no coincide")]
    DestinoPromocionNoCoincide,
    #[error("la candidata no supera todas las comprobaciones")]
    CandidataNoValida,
    #[error("faltan los dos ciclos de validación")]
    CiclosInsuficientes,
    #[error("el ciclo no conserva inicio y apagado observados")]
    CicloIncompleto,
    #[error("el ciclo no conserva una comprobación de identidad SSH válida")]
    IdentidadSshCicloNoValidada,
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

    #[test]
    fn exige_y_conserva_identidad_ssh_tipada_en_cada_ciclo() {
        let mut valor = recibo();
        valor.iniciar(2).unwrap();
        valor.detener(3).unwrap();
        valor
            .registrar_saneamiento(Identificador::nuevo("windows-nueva").unwrap(), 4)
            .unwrap();
        valor.iniciar_ciclo_plantilla(5).unwrap();
        assert_eq!(
            valor.detener(6).unwrap_err(),
            ErrorReserva::IdentidadSshCicloNoValidada
        );
        assert_eq!(
            valor
                .registrar_identidad_ssh_ciclo(
                    ComprobacionIdentidadSsh {
                        aplicable: true,
                        correcta: false,
                        observada_en_unix_ms: Some(6),
                        huella_sha256: Some(
                            "SHA256:m/ejOXkI0ofKT+bW34mhHdSH63xu9c+QU7djuKirDXM".to_owned(),
                        ),
                    },
                    6,
                )
                .unwrap_err(),
            ErrorReserva::IdentidadSshCicloNoValidada
        );
        valor
            .registrar_identidad_ssh_ciclo(
                ComprobacionIdentidadSsh::validada(
                    7,
                    "SHA256:m/ejOXkI0ofKT+bW34mhHdSH63xu9c+QU7djuKirDXM",
                ),
                7,
            )
            .unwrap();
        valor.detener(8).unwrap();
        let diagnostico = crate::dominio::preparacion_plantilla::diagnosticar_candidata(
            crate::dominio::preparacion_plantilla::EstadoCandidataPlantilla {
                apagada: true,
                sin_medios_temporales: true,
                red_final_conforme: true,
                disco_sistema_unico: true,
                identidad_independiente: true,
            },
        );
        valor.registrar_validacion_ciclo(diagnostico, 9).unwrap();
        let ciclo = &valor
            .preparacion_plantilla
            .as_ref()
            .unwrap()
            .ciclos_validados[0];
        assert_eq!(
            ciclo.identidad_ssh,
            ComprobacionIdentidadSsh::validada(
                7,
                "SHA256:m/ejOXkI0ofKT+bW34mhHdSH63xu9c+QU7djuKirDXM"
            )
        );
    }

    #[test]
    fn produce_un_identificador_de_instancia_valido_desde_el_maximo_publico() {
        let id_ejecucion = Identificador::nuevo("a".repeat(64)).unwrap();
        let valor = ReciboReserva::nuevo(
            id_ejecucion.clone(),
            Identificador::nuevo("windows-analisis").unwrap(),
            SistemaInvitado::Windows,
            1,
        )
        .unwrap();
        assert_eq!(valor.id_ejecucion, id_ejecucion);
        assert_eq!(valor.id_instancia.como_str().len(), 64);
        assert!(valor.id_instancia.como_str().starts_with("lab-"));
    }
}
