// Laboratorio de máquinas virtuales con libvirt
// Copyright (C) 2026 Alberto Avidad
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::dominio::acceso::EstadoAccesoObservado;
use crate::dominio::arranque::EstadoArranqueObservado;
use crate::dominio::identificador::Identificador;
use crate::dominio::plantilla::{EstadoPlantilla, IdentidadServidor, Plantilla};
use crate::dominio::preparacion_plantilla::{DestinoPromocion, EstadoCandidataPlantilla};
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

/// Inventario privado de destinos preautorizados. Un consumidor nunca aporta
/// nombres, rutas, XML, redes ni volúmenes del hipervisor.
pub trait CatalogoDestinosPromocion {
    fn obtener_destino(&self, id: &Identificador) -> Result<DestinoPromocion>;
    fn listar_destinos(&self) -> Result<Vec<DestinoPromocion>>;
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
    fn identidad_servidor(
        &self,
        plantilla: &Plantilla,
        recibo: &ReciboReserva,
    ) -> Result<Option<IdentidadServidor>>;
    fn retirar_instancia(&self, plantilla: &Plantilla, id_instancia: &Identificador) -> Result<()>;
}

/// Observa únicamente señales acotadas de acceso y las reduce a booleanos.
/// Los detalles del hipervisor no atraviesan este puerto.
pub trait ObservadorAcceso {
    fn observar_acceso(
        &self,
        plantilla: &Plantilla,
        recibo: &ReciboReserva,
    ) -> EstadoAccesoObservado;
}

/// Diagnóstico de arranque de solo lectura. El adaptador reduce definición,
/// almacenamiento, redes y el último fallo a señales cerradas.
pub trait ObservadorArranque {
    fn observar_arranque(&self, recibo: &ReciboReserva) -> Result<EstadoArranqueObservado>;
}

/// Operaciones acotadas para convertir una reserva en una nueva plantilla.
/// El adaptador resuelve todos los recursos desde inventario privado.
pub trait ProveedorPreparacionPlantillas {
    fn comprobar_destino_libre(&self, destino: &DestinoPromocion) -> Result<()>;
    fn sanear_candidata(&self, recibo: &ReciboReserva, destino: &DestinoPromocion) -> Result<()>;
    fn validar_identidad_ssh_candidata(
        &self,
        recibo: &ReciboReserva,
        destino: &DestinoPromocion,
    ) -> Result<IdentidadServidor>;
    fn inspeccionar_candidata(
        &self,
        recibo: &ReciboReserva,
        destino: &DestinoPromocion,
    ) -> Result<EstadoCandidataPlantilla>;
    fn promover_candidata(&self, recibo: &ReciboReserva, destino: &DestinoPromocion) -> Result<()>;
}

/// Persistencia de recibos con reserva exclusiva del identificador y
/// actualización por revisión. Las escrituras deben ser atómicas.
pub trait AlmacenRecibosReserva {
    fn bloquear_preparacion(&self) -> Result<Box<dyn GuardiaPreparacion + '_>>;
    fn bloquear_mutacion(
        &self,
        id_ejecucion: &Identificador,
    ) -> Result<Box<dyn GuardiaMutacion + '_>>;
    fn existe(&self, id_ejecucion: &Identificador) -> Result<bool>;
    fn guardar_nuevo(&self, recibo: &ReciboReserva) -> Result<()>;
    fn cargar(&self, id_ejecucion: &Identificador) -> Result<ReciboReserva>;
    fn listar(&self) -> Result<Vec<ReciboReserva>>;
    fn actualizar(&self, recibo: &ReciboReserva) -> Result<()>;
}

/// Guardia opaca que serializa la comprobación de cuota y la reserva de un
/// identificador entre procesos distintos.
pub trait GuardiaPreparacion {}

/// Serializa el efecto externo y la persistencia de una mutación completa de
/// una reserva, no solo la escritura final del recibo.
pub trait GuardiaMutacion {}

/// Comprueba resultados copiados fuera de la máquina no confiable.
pub trait VerificadorResultados {
    fn verificar(&self, id_ejecucion: &Identificador) -> Result<ResultadosProtegidos>;
}

pub trait Reloj {
    fn ahora_unix_ms(&self) -> u64;
}
