// Laboratorio de máquinas virtuales con libvirt
// Copyright (C) 2026 Alberto Avidad
// SPDX-License-Identifier: GPL-3.0-or-later

use serde::Serialize;

/// Resultado saneado del proveedor. No contiene direcciones, MAC, nombres,
/// rutas ni salida de herramientas del hipervisor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EstadoAccesoObservado {
    pub instancia_encendida: bool,
    pub direccion_lease_observable: bool,
    pub direccion_agente_observable: bool,
    pub direccion_arp_observable: bool,
    pub qga_disponible: bool,
    pub marca_uuid_presente: bool,
    pub marca_uuid_coincidente: bool,
    pub clave_servidor_ssh_presente: bool,
    pub clave_servidor_ssh_valida: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CodigoComprobacionAcceso {
    ReservaEnEjecucion,
    InstanciaEncendida,
    DireccionObservable,
    DireccionLeaseObservable,
    DireccionAgenteObservable,
    DireccionArpObservable,
    QgaDisponible,
    MarcaUuidPresente,
    MarcaUuidCoincidente,
    ClaveServidorSshPresente,
    ClaveServidorSshValida,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ComprobacionAcceso {
    pub codigo: CodigoComprobacionAcceso,
    pub aplicable: bool,
    /// Solo una comprobación aplicable y bloqueante puede impedir que el
    /// diagnóstico quede preparado. Las fuentes alternativas se publican
    /// como información, no como fallos contradictorios.
    pub bloqueante: bool,
    pub correcta: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiagnosticoAcceso {
    pub preparado: bool,
    pub comprobaciones: Vec<ComprobacionAcceso>,
}

pub fn diagnosticar_acceso(
    reserva_en_ejecucion: bool,
    exige_identidad_ssh: bool,
    estado: EstadoAccesoObservado,
) -> DiagnosticoAcceso {
    let direccion_observable = estado.direccion_lease_observable
        || estado.direccion_agente_observable
        || estado.direccion_arp_observable;
    let identidad_correcta = !exige_identidad_ssh
        || (estado.qga_disponible
            && estado.marca_uuid_presente
            && estado.marca_uuid_coincidente
            && estado.clave_servidor_ssh_presente
            && estado.clave_servidor_ssh_valida);
    let comprobaciones = vec![
        comprobacion(
            CodigoComprobacionAcceso::ReservaEnEjecucion,
            true,
            true,
            reserva_en_ejecucion,
        ),
        comprobacion(
            CodigoComprobacionAcceso::InstanciaEncendida,
            true,
            true,
            estado.instancia_encendida,
        ),
        comprobacion(
            CodigoComprobacionAcceso::DireccionObservable,
            true,
            true,
            direccion_observable,
        ),
        comprobacion(
            CodigoComprobacionAcceso::DireccionLeaseObservable,
            true,
            false,
            estado.direccion_lease_observable,
        ),
        comprobacion(
            CodigoComprobacionAcceso::DireccionAgenteObservable,
            true,
            false,
            estado.direccion_agente_observable,
        ),
        comprobacion(
            CodigoComprobacionAcceso::DireccionArpObservable,
            true,
            false,
            estado.direccion_arp_observable,
        ),
        comprobacion(
            CodigoComprobacionAcceso::QgaDisponible,
            exige_identidad_ssh,
            exige_identidad_ssh,
            estado.qga_disponible,
        ),
        comprobacion(
            CodigoComprobacionAcceso::MarcaUuidPresente,
            exige_identidad_ssh,
            exige_identidad_ssh,
            estado.marca_uuid_presente,
        ),
        comprobacion(
            CodigoComprobacionAcceso::MarcaUuidCoincidente,
            exige_identidad_ssh,
            exige_identidad_ssh,
            estado.marca_uuid_coincidente,
        ),
        comprobacion(
            CodigoComprobacionAcceso::ClaveServidorSshPresente,
            exige_identidad_ssh,
            exige_identidad_ssh,
            estado.clave_servidor_ssh_presente,
        ),
        comprobacion(
            CodigoComprobacionAcceso::ClaveServidorSshValida,
            exige_identidad_ssh,
            exige_identidad_ssh,
            estado.clave_servidor_ssh_valida,
        ),
    ];
    DiagnosticoAcceso {
        preparado: reserva_en_ejecucion
            && estado.instancia_encendida
            && direccion_observable
            && identidad_correcta,
        comprobaciones,
    }
}

fn comprobacion(
    codigo: CodigoComprobacionAcceso,
    aplicable: bool,
    bloqueante: bool,
    correcta: bool,
) -> ComprobacionAcceso {
    ComprobacionAcceso {
        codigo,
        aplicable,
        bloqueante: aplicable && bloqueante,
        correcta: aplicable && correcta,
    }
}

#[cfg(test)]
mod pruebas {
    use super::*;

    #[test]
    fn acepta_cualquier_fuente_de_direccion_y_exige_identidad_ssh() {
        let diagnostico = diagnosticar_acceso(
            true,
            true,
            EstadoAccesoObservado {
                instancia_encendida: true,
                direccion_lease_observable: false,
                direccion_agente_observable: false,
                direccion_arp_observable: true,
                qga_disponible: true,
                marca_uuid_presente: true,
                marca_uuid_coincidente: true,
                clave_servidor_ssh_presente: true,
                clave_servidor_ssh_valida: true,
            },
        );
        assert!(diagnostico.preparado);
        assert!(diagnostico
            .comprobaciones
            .iter()
            .filter(|valor| valor.bloqueante)
            .all(|valor| valor.correcta));
        assert!(diagnostico.comprobaciones.iter().any(|valor| {
            valor.codigo == CodigoComprobacionAcceso::DireccionArpObservable
                && valor.correcta
                && !valor.bloqueante
        }));
    }

    #[test]
    fn marca_identidad_no_aplicable_fuera_de_ssh() {
        let diagnostico = diagnosticar_acceso(
            true,
            false,
            EstadoAccesoObservado {
                instancia_encendida: true,
                direccion_lease_observable: true,
                direccion_agente_observable: false,
                direccion_arp_observable: false,
                qga_disponible: false,
                marca_uuid_presente: false,
                marca_uuid_coincidente: false,
                clave_servidor_ssh_presente: false,
                clave_servidor_ssh_valida: false,
            },
        );
        assert!(diagnostico.preparado);
        assert!(diagnostico.comprobaciones[6..]
            .iter()
            .all(|valor| !valor.aplicable && !valor.bloqueante && !valor.correcta));
    }
}
