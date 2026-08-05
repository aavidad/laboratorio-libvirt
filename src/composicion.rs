// Laboratorio de máquinas virtuales con libvirt
// Copyright (C) 2026 Alberto Avidad
// SPDX-License-Identifier: GPL-3.0-or-later

//! Raíz de composición. Es el único módulo que ensambla casos de uso con
//! adaptadores concretos.

use crate::adaptadores::configuracion_json::ConfiguracionLocal;
use crate::adaptadores::libvirt_cli::LibvirtCli;
use crate::adaptadores::recibos_json::{AlmacenRecibosJson, RelojSistema};
use crate::adaptadores::resultados_locales::VerificadorResultadosLocales;
use crate::aplicacion::gestionar_preparacion_plantilla::GestorPreparacionPlantillas;
use crate::aplicacion::gestionar_reserva::{
    diagnosticar_acceso_reserva, diagnosticar_arranque_reserva, inspeccionar_plantilla,
    GestorReservas,
};
use crate::aplicacion::ordenes::{Orden, Respuesta};
use crate::aplicacion::puertos::{
    AlmacenRecibosReserva, CatalogoDestinosPromocion, CatalogoPlantillas,
};
use anyhow::Result;

pub fn ejecutar(configuracion: ConfiguracionLocal, orden: Orden) -> Result<Respuesta> {
    match orden {
        Orden::ListarPlantillas => Ok(Respuesta::Plantillas(configuracion.listar()?)),
        Orden::ListarReservas => {
            let almacen = AlmacenRecibosJson::nuevo(&configuracion.raiz_recibos)?;
            Ok(Respuesta::Reservas(almacen.listar()?))
        }
        Orden::ListarDestinosPromocion => Ok(Respuesta::DestinosPromocion(
            configuracion.listar_destinos()?,
        )),
        Orden::Inspeccionar { id_plantilla } => {
            let proveedor = construir_proveedor(&configuracion)?;
            Ok(Respuesta::Diagnostico(inspeccionar_plantilla(
                &configuracion,
                &proveedor,
                &id_plantilla,
            )?))
        }
        Orden::Estado { id_ejecucion } => {
            let almacen = AlmacenRecibosJson::nuevo(&configuracion.raiz_recibos)?;
            Ok(Respuesta::Recibo(almacen.cargar(&id_ejecucion)?))
        }
        Orden::Acceso { id_ejecucion } => {
            let proveedor = construir_proveedor(&configuracion)?;
            let almacen = AlmacenRecibosJson::nuevo(&configuracion.raiz_recibos)?;
            let resultados = VerificadorResultadosLocales::con_cuotas(
                &configuracion.raiz_resultados,
                configuracion.cuotas_resultados,
            )?;
            let gestor = GestorReservas::nuevo(
                configuracion.clone(),
                proveedor,
                almacen,
                resultados,
                RelojSistema,
                configuracion.maximo_reservas_activas,
            )?;
            Ok(Respuesta::Acceso(gestor.obtener_acceso(&id_ejecucion)?))
        }
        Orden::DiagnosticarAcceso { id_ejecucion } => {
            let proveedor = construir_proveedor(&configuracion)?;
            let almacen = AlmacenRecibosJson::nuevo(&configuracion.raiz_recibos)?;
            Ok(Respuesta::DiagnosticoAcceso(diagnosticar_acceso_reserva(
                &configuracion,
                &proveedor,
                &almacen,
                &id_ejecucion,
            )?))
        }
        Orden::DiagnosticarArranque { id_ejecucion } => {
            let proveedor = construir_proveedor(&configuracion)?;
            let almacen = AlmacenRecibosJson::nuevo(&configuracion.raiz_recibos)?;
            Ok(Respuesta::DiagnosticoArranque(
                diagnosticar_arranque_reserva(&proveedor, &almacen, &id_ejecucion)?,
            ))
        }
        otra => ejecutar_mutacion(configuracion, otra),
    }
}

fn ejecutar_mutacion(configuracion: ConfiguracionLocal, orden: Orden) -> Result<Respuesta> {
    if matches!(
        orden,
        Orden::SanearPlantilla { .. }
            | Orden::IniciarCicloPlantilla { .. }
            | Orden::DetenerCicloPlantilla { .. }
            | Orden::ValidarCicloPlantilla { .. }
            | Orden::PromoverPlantilla { .. }
    ) {
        return ejecutar_mutacion_preparacion(configuracion, orden);
    }
    let proveedor = construir_proveedor(&configuracion)?;
    let almacen = AlmacenRecibosJson::nuevo(&configuracion.raiz_recibos)?;
    let resultados = VerificadorResultadosLocales::con_cuotas(
        &configuracion.raiz_resultados,
        configuracion.cuotas_resultados,
    )?;
    let gestor = GestorReservas::nuevo(
        configuracion.clone(),
        proveedor,
        almacen,
        resultados,
        RelojSistema,
        configuracion.maximo_reservas_activas,
    )?;
    let recibo = match orden {
        Orden::Preparar {
            id_plantilla,
            id_ejecucion,
            confirmacion,
        } => gestor.preparar(id_plantilla, id_ejecucion, &confirmacion)?,
        Orden::Iniciar {
            id_ejecucion,
            confirmacion,
        } => gestor.iniciar(&id_ejecucion, &confirmacion)?,
        Orden::Reconciliar {
            id_ejecucion,
            confirmacion,
        } => gestor.reconciliar(&id_ejecucion, &confirmacion)?,
        Orden::Detener {
            id_ejecucion,
            confirmacion,
        } => gestor.detener(&id_ejecucion, &confirmacion)?,
        Orden::ProtegerResultados {
            id_ejecucion,
            confirmacion,
        } => gestor.proteger_resultados(&id_ejecucion, &confirmacion)?,
        Orden::MarcarFallida {
            id_ejecucion,
            motivo,
            confirmacion,
        } => gestor.marcar_fallida(&id_ejecucion, motivo, &confirmacion)?,
        Orden::Descartar {
            id_ejecucion,
            confirmacion,
        } => gestor.descartar(&id_ejecucion, &confirmacion)?,
        Orden::DescartarFallida {
            id_ejecucion,
            confirmacion,
            acepta_perdida_resultados,
        } => gestor.descartar_fallida(&id_ejecucion, &confirmacion, acepta_perdida_resultados)?,
        Orden::ListarPlantillas
        | Orden::ListarReservas
        | Orden::ListarDestinosPromocion
        | Orden::Inspeccionar { .. }
        | Orden::Estado { .. }
        | Orden::Acceso { .. }
        | Orden::DiagnosticarAcceso { .. }
        | Orden::DiagnosticarArranque { .. }
        | Orden::SanearPlantilla { .. }
        | Orden::IniciarCicloPlantilla { .. }
        | Orden::DetenerCicloPlantilla { .. }
        | Orden::ValidarCicloPlantilla { .. }
        | Orden::PromoverPlantilla { .. } => {
            unreachable!("las consultas se despachan antes de construir los adaptadores mutables")
        }
    };
    Ok(Respuesta::Recibo(recibo))
}

fn ejecutar_mutacion_preparacion(
    configuracion: ConfiguracionLocal,
    orden: Orden,
) -> Result<Respuesta> {
    let proveedor = construir_proveedor(&configuracion)?;
    let almacen = AlmacenRecibosJson::nuevo(&configuracion.raiz_recibos)?;
    let gestor =
        GestorPreparacionPlantillas::nuevo(configuracion, proveedor, almacen, RelojSistema);
    let recibo = match orden {
        Orden::SanearPlantilla {
            id_ejecucion,
            id_destino,
            confirmacion,
        } => gestor.sanear(&id_ejecucion, &id_destino, &confirmacion)?,
        Orden::IniciarCicloPlantilla {
            id_ejecucion,
            confirmacion,
        } => gestor.iniciar_ciclo(&id_ejecucion, &confirmacion)?,
        Orden::DetenerCicloPlantilla {
            id_ejecucion,
            confirmacion,
        } => gestor.detener_ciclo(&id_ejecucion, &confirmacion)?,
        Orden::ValidarCicloPlantilla {
            id_ejecucion,
            confirmacion,
        } => gestor.validar_ciclo(&id_ejecucion, &confirmacion)?,
        Orden::PromoverPlantilla {
            id_ejecucion,
            confirmacion,
        } => gestor.promover(&id_ejecucion, &confirmacion)?,
        _ => unreachable!("solo se despachan órdenes tipadas de preparación"),
    };
    Ok(Respuesta::Recibo(recibo))
}

fn construir_proveedor(configuracion: &ConfiguracionLocal) -> Result<LibvirtCli> {
    LibvirtCli::nuevo(
        configuracion.uri_libvirt.clone(),
        configuracion.definiciones_libvirt(),
        configuracion.definiciones_promocion(),
    )
}
