// Laboratorio de máquinas virtuales con libvirt
// Copyright (C) 2026 Alberto Avidad
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::dominio::identificador::Identificador;
use crate::dominio::plantilla::{DiagnosticoPlantilla, Plantilla, PuntoAcceso};
use crate::dominio::reserva::{MotivoFallo, ReciboReserva};
use serde::Serialize;

#[derive(Debug, Clone)]
pub enum Orden {
    ListarPlantillas,
    ListarReservas,
    Inspeccionar {
        id_plantilla: Identificador,
    },
    Estado {
        id_ejecucion: Identificador,
    },
    Acceso {
        id_ejecucion: Identificador,
    },
    Preparar {
        id_plantilla: Identificador,
        id_ejecucion: Identificador,
        confirmacion: Identificador,
    },
    Iniciar {
        id_ejecucion: Identificador,
        confirmacion: Identificador,
    },
    Reconciliar {
        id_ejecucion: Identificador,
        confirmacion: Identificador,
    },
    Detener {
        id_ejecucion: Identificador,
        confirmacion: Identificador,
    },
    ProtegerResultados {
        id_ejecucion: Identificador,
        confirmacion: Identificador,
    },
    MarcarFallida {
        id_ejecucion: Identificador,
        motivo: MotivoFallo,
        confirmacion: Identificador,
    },
    Descartar {
        id_ejecucion: Identificador,
        confirmacion: Identificador,
    },
    DescartarFallida {
        id_ejecucion: Identificador,
        confirmacion: Identificador,
        acepta_perdida_resultados: bool,
    },
}

#[derive(Debug, Serialize)]
#[serde(tag = "tipo", content = "datos", rename_all = "snake_case")]
pub enum Respuesta {
    Plantillas(Vec<Plantilla>),
    Reservas(Vec<ReciboReserva>),
    Diagnostico(DiagnosticoPlantilla),
    Recibo(ReciboReserva),
    Acceso(PuntoAcceso),
}
