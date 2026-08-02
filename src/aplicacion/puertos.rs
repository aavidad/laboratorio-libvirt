// Laboratorio de máquinas virtuales con libvirt
// Copyright (C) 2026 Alberto Avidad
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::dominio::identificador::Identificador;
use crate::dominio::plantilla::{EstadoPlantilla, Plantilla};
use crate::dominio::reserva::{ReciboReserva, ResultadosProtegidos};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::time::Duration;

/// Catálogo público de plantillas. Solo expone metadatos y capacidades; las
/// referencias de libvirt pertenecen al adaptador correspondiente.
pub trait CatalogoPlantillas {
    fn obtener(&self, id: &Identificador) -> Result<Plantilla>;
    fn listar(&self) -> Result<Vec<Plantilla>>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EstadoInstancia {
    Ausente,
    Apagada,
    Encendida,
}

/// Puerto del proveedor de máquinas. No presupone libvirt, QEMU, una nube ni
/// una familia concreta de sistema invitado.
pub trait ProveedorInstancias {
    fn inspeccionar_plantilla(&self, plantilla: &Plantilla) -> Result<EstadoPlantilla>;
    fn preparar_instancia(&self, plantilla: &Plantilla, recibo: &ReciboReserva) -> Result<()>;
    fn estado_instancia(&self, id_instancia: &Identificador) -> Result<EstadoInstancia>;
    fn iniciar_instancia(&self, id_instancia: &Identificador) -> Result<()>;
    fn solicitar_apagado(&self, id_instancia: &Identificador) -> Result<()>;
    fn esperar_apagado(&self, id_instancia: &Identificador, plazo: Duration) -> Result<bool>;
    fn direccion_instancia(&self, id_instancia: &Identificador) -> Result<Option<IpAddr>>;
    fn retirar_instancia(&self, plantilla: &Plantilla, id_instancia: &Identificador) -> Result<()>;
}

/// Persistencia de recibos con reserva exclusiva del identificador y
/// actualización por revisión. Las escrituras deben ser atómicas.
pub trait AlmacenRecibosReserva {
    fn bloquear_preparacion(&self) -> Result<Box<dyn GuardiaPreparacion + '_>>;
    fn existe(&self, id_ejecucion: &Identificador) -> Result<bool>;
    fn guardar_nuevo(&self, recibo: &ReciboReserva) -> Result<()>;
    fn cargar(&self, id_ejecucion: &Identificador) -> Result<ReciboReserva>;
    fn listar(&self) -> Result<Vec<ReciboReserva>>;
    fn actualizar(&self, recibo: &ReciboReserva) -> Result<()>;
}

/// Guardia opaca que serializa la comprobación de cuota y la reserva de un
/// identificador entre procesos distintos.
pub trait GuardiaPreparacion {}

/// Comprueba resultados copiados fuera de la máquina no confiable.
pub trait VerificadorResultados {
    fn verificar(&self, id_ejecucion: &Identificador) -> Result<ResultadosProtegidos>;
}

pub trait Reloj {
    fn ahora_unix_ms(&self) -> u64;
}
