// Laboratorio de máquinas virtuales con libvirt
// Copyright (C) 2026 Alberto Avidad
// SPDX-License-Identifier: GPL-3.0-or-later

use serde::Serialize;

/// Señales acotadas observadas antes de un arranque. No contiene referencias
/// del hipervisor ni texto procedente de sus registros.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EstadoArranqueObservado {
    pub instancia_registrada: bool,
    pub instancia_apagada: bool,
    pub definicion_valida: bool,
    pub almacenamiento_presente: bool,
    pub recursos_escritura_disponibles: bool,
    pub redes_requeridas_activas: bool,
    pub estado_guardado_ausente: bool,
    pub ultimo_estado_fallido: bool,
    pub ultimo_fallo: Option<CodigoFalloArranque>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CodigoComprobacionArranque {
    ReservaIniciable,
    InstanciaRegistrada,
    InstanciaApagada,
    DefinicionValida,
    AlmacenamientoPresente,
    RecursosEscrituraDisponibles,
    RedesRequeridasActivas,
    EstadoGuardadoAusente,
}

impl CodigoComprobacionArranque {
    pub const fn codigo(self) -> &'static str {
        match self {
            Self::ReservaIniciable => "reserva_iniciable",
            Self::InstanciaRegistrada => "instancia_registrada",
            Self::InstanciaApagada => "instancia_apagada",
            Self::DefinicionValida => "definicion_valida",
            Self::AlmacenamientoPresente => "almacenamiento_presente",
            Self::RecursosEscrituraDisponibles => "recursos_escritura_disponibles",
            Self::RedesRequeridasActivas => "redes_requeridas_activas",
            Self::EstadoGuardadoAusente => "estado_guardado_ausente",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CodigoFalloArranque {
    BloqueoEscrituraRecurso,
    PermisoRecursoDenegado,
    RecursoRequeridoAusente,
    RedRequeridaInactiva,
    FirmwareNoDisponible,
    TpmNoDisponible,
    EspacioInsuficiente,
    ConfiguracionNoAdmitida,
    NoClasificado,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ComprobacionArranque {
    pub codigo: CodigoComprobacionArranque,
    pub correcta: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiagnosticoArranque {
    pub iniciable: bool,
    /// Código cerrado del fallo más reciente, si el hipervisor conserva una
    /// razón saneable. Es informativo: las comprobaciones describen el estado
    /// actual y permiten saber si un reintento ya es viable.
    pub ultimo_fallo: Option<CodigoFalloArranque>,
    pub ultimo_estado_fallido: bool,
    pub comprobaciones: Vec<ComprobacionArranque>,
}

pub fn diagnosticar_arranque(
    reserva_iniciable: bool,
    estado: EstadoArranqueObservado,
) -> DiagnosticoArranque {
    let comprobaciones = vec![
        comprobacion(
            CodigoComprobacionArranque::ReservaIniciable,
            reserva_iniciable,
        ),
        comprobacion(
            CodigoComprobacionArranque::InstanciaRegistrada,
            estado.instancia_registrada,
        ),
        comprobacion(
            CodigoComprobacionArranque::InstanciaApagada,
            estado.instancia_apagada,
        ),
        comprobacion(
            CodigoComprobacionArranque::DefinicionValida,
            estado.definicion_valida,
        ),
        comprobacion(
            CodigoComprobacionArranque::AlmacenamientoPresente,
            estado.almacenamiento_presente,
        ),
        comprobacion(
            CodigoComprobacionArranque::RecursosEscrituraDisponibles,
            estado.recursos_escritura_disponibles,
        ),
        comprobacion(
            CodigoComprobacionArranque::RedesRequeridasActivas,
            estado.redes_requeridas_activas,
        ),
        comprobacion(
            CodigoComprobacionArranque::EstadoGuardadoAusente,
            estado.estado_guardado_ausente,
        ),
    ];
    DiagnosticoArranque {
        iniciable: comprobaciones.iter().all(|valor| valor.correcta),
        ultimo_fallo: estado.ultimo_fallo.or_else(|| {
            estado
                .ultimo_estado_fallido
                .then_some(CodigoFalloArranque::NoClasificado)
        }),
        ultimo_estado_fallido: estado.ultimo_estado_fallido,
        comprobaciones,
    }
}

fn comprobacion(codigo: CodigoComprobacionArranque, correcta: bool) -> ComprobacionArranque {
    ComprobacionArranque { codigo, correcta }
}

#[cfg(test)]
mod pruebas {
    use super::*;

    #[test]
    fn separa_el_fallo_historico_del_estado_actual_iniciable() {
        let diagnostico = diagnosticar_arranque(
            true,
            EstadoArranqueObservado {
                instancia_registrada: true,
                instancia_apagada: true,
                definicion_valida: true,
                almacenamiento_presente: true,
                recursos_escritura_disponibles: true,
                redes_requeridas_activas: true,
                estado_guardado_ausente: true,
                ultimo_estado_fallido: true,
                ultimo_fallo: Some(CodigoFalloArranque::BloqueoEscrituraRecurso),
            },
        );
        assert!(diagnostico.iniciable);
        assert_eq!(
            diagnostico.ultimo_fallo,
            Some(CodigoFalloArranque::BloqueoEscrituraRecurso)
        );
    }

    #[test]
    fn un_recurso_de_escritura_bloqueado_impide_el_arranque() {
        let diagnostico = diagnosticar_arranque(
            true,
            EstadoArranqueObservado {
                instancia_registrada: true,
                instancia_apagada: true,
                definicion_valida: true,
                almacenamiento_presente: true,
                recursos_escritura_disponibles: false,
                redes_requeridas_activas: true,
                estado_guardado_ausente: true,
                ultimo_estado_fallido: true,
                ultimo_fallo: Some(CodigoFalloArranque::BloqueoEscrituraRecurso),
            },
        );
        assert!(!diagnostico.iniciable);
        assert!(diagnostico.comprobaciones.iter().any(|comprobacion| {
            comprobacion.codigo == CodigoComprobacionArranque::RecursosEscrituraDisponibles
                && !comprobacion.correcta
        }));
    }
}
