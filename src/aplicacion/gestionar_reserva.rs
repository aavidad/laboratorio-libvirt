// Laboratorio de máquinas virtuales con libvirt
// Copyright (C) 2026 Alberto Avidad
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::aplicacion::puertos::{
    AlmacenRecibosReserva, CatalogoPlantillas, EstadoInstancia, ProveedorInstancias, Reloj,
    VerificadorResultados,
};
use crate::dominio::identificador::Identificador;
use crate::dominio::plantilla::{diagnosticar, DiagnosticoPlantilla, PuntoAcceso};
use crate::dominio::reserva::{EstadoReserva, MotivoFallo, ReciboReserva};
use anyhow::{bail, Context, Result};
use std::time::Duration;

pub const PLAZO_APAGADO: Duration = Duration::from_secs(30);

/// Casos de uso del ciclo de vida. El tipo solo conoce puertos; no conoce
/// libvirt, JSON, HTTP, terminales ni detalles de Windows o Linux.
pub struct GestorReservas<C, P, A, V, R> {
    catalogo: C,
    proveedor: P,
    almacen: A,
    resultados: V,
    reloj: R,
    maximo_reservas_activas: usize,
}

pub fn inspeccionar_plantilla<C, P>(
    catalogo: &C,
    proveedor: &P,
    id_plantilla: &Identificador,
) -> Result<DiagnosticoPlantilla>
where
    C: CatalogoPlantillas,
    P: ProveedorInstancias,
{
    let plantilla = catalogo.obtener(id_plantilla)?;
    let estado = proveedor.inspeccionar_plantilla(&plantilla)?;
    Ok(diagnosticar(&estado))
}

impl<C, P, A, V, R> GestorReservas<C, P, A, V, R>
where
    C: CatalogoPlantillas,
    P: ProveedorInstancias,
    A: AlmacenRecibosReserva,
    V: VerificadorResultados,
    R: Reloj,
{
    pub fn nuevo(
        catalogo: C,
        proveedor: P,
        almacen: A,
        resultados: V,
        reloj: R,
        maximo_reservas_activas: usize,
    ) -> Result<Self> {
        if !(1..=128).contains(&maximo_reservas_activas) {
            bail!("el máximo de reservas activas debe estar entre 1 y 128");
        }
        Ok(Self {
            catalogo,
            proveedor,
            almacen,
            resultados,
            reloj,
            maximo_reservas_activas,
        })
    }

    pub fn preparar(
        &self,
        id_plantilla: Identificador,
        id_ejecucion: Identificador,
        confirmacion: &Identificador,
    ) -> Result<ReciboReserva> {
        exigir_confirmacion(&id_ejecucion, confirmacion)?;
        let bloqueo_preparacion = self.almacen.bloquear_preparacion()?;
        if self.almacen.existe(&id_ejecucion)? {
            bail!("el identificador de ejecución ya está reservado");
        }
        let activas = self
            .almacen
            .listar()?
            .into_iter()
            .filter(|recibo| recibo.estado != EstadoReserva::Descartada)
            .count();
        if activas >= self.maximo_reservas_activas {
            bail!("se alcanzó la cuota de reservas activas");
        }
        let plantilla = self.catalogo.obtener(&id_plantilla)?;
        let diagnostico = inspeccionar_plantilla(&self.catalogo, &self.proveedor, &id_plantilla)?;
        if !diagnostico.preparada {
            let reglas = diagnostico
                .comprobaciones
                .iter()
                .filter(|valor| !valor.correcta)
                .map(|valor| valor.codigo.codigo())
                .collect::<Vec<_>>()
                .join(", ");
            bail!("la plantilla no está preparada: {reglas}");
        }
        let recibo = ReciboReserva::nuevo(
            id_ejecucion.clone(),
            id_plantilla,
            plantilla.sistema,
            self.reloj.ahora_unix_ms(),
        )?;
        self.almacen.guardar_nuevo(&recibo)?;
        drop(bloqueo_preparacion);
        if let Err(error) = self.proveedor.preparar_instancia(&plantilla, &recibo) {
            let mut recibo_fallido = recibo;
            recibo_fallido
                .marcar_fallida(MotivoFallo::Infraestructura, self.reloj.ahora_unix_ms())
                .context("no se pudo registrar el fallo de preparación")?;
            self.almacen
                .actualizar(&recibo_fallido)
                .context("falló la preparación y tampoco pudo persistirse su estado fallido")?;
            return Err(error).context("no se pudo preparar la instancia efímera");
        }
        Ok(recibo)
    }

    pub fn iniciar(
        &self,
        id_ejecucion: &Identificador,
        confirmacion: &Identificador,
    ) -> Result<ReciboReserva> {
        exigir_confirmacion(id_ejecucion, confirmacion)?;
        let mut recibo = self.almacen.cargar(id_ejecucion)?;
        if self.proveedor.estado_instancia(&recibo.id_instancia)? != EstadoInstancia::Apagada {
            bail!("la instancia no está apagada y preparada");
        }
        self.proveedor.iniciar_instancia(&recibo.id_instancia)?;
        recibo.iniciar(self.reloj.ahora_unix_ms())?;
        self.almacen.actualizar(&recibo)?;
        Ok(recibo)
    }

    /// Repara únicamente las dos ventanas de interrupción inequívocas: la VM
    /// arrancó antes de persistir `en_ejecucion`, o terminó de apagarse antes
    /// de persistir `detenida`. No interpreta otros estados por conjetura.
    pub fn reconciliar(
        &self,
        id_ejecucion: &Identificador,
        confirmacion: &Identificador,
    ) -> Result<ReciboReserva> {
        exigir_confirmacion(id_ejecucion, confirmacion)?;
        let mut recibo = self.almacen.cargar(id_ejecucion)?;
        let estado_instancia = self.proveedor.estado_instancia(&recibo.id_instancia)?;
        let modificada = match (recibo.estado, estado_instancia) {
            (EstadoReserva::Preparada, EstadoInstancia::Encendida) => {
                recibo.iniciar(self.reloj.ahora_unix_ms())?;
                true
            }
            (EstadoReserva::EnEjecucion, EstadoInstancia::Apagada) => {
                recibo.detener(self.reloj.ahora_unix_ms())?;
                true
            }
            (EstadoReserva::Preparada | EstadoReserva::Detenida, EstadoInstancia::Apagada)
            | (EstadoReserva::EnEjecucion, EstadoInstancia::Encendida)
            | (
                EstadoReserva::ResultadosProtegidos | EstadoReserva::Fallida,
                EstadoInstancia::Apagada | EstadoInstancia::Ausente,
            )
            | (EstadoReserva::Descartada, EstadoInstancia::Ausente) => false,
            _ => bail!("el estado observado no admite una reconciliación automática segura"),
        };
        if modificada {
            self.almacen.actualizar(&recibo)?;
        }
        Ok(recibo)
    }

    pub fn obtener_acceso(&self, id_ejecucion: &Identificador) -> Result<PuntoAcceso> {
        let recibo = self.almacen.cargar(id_ejecucion)?;
        if recibo.estado != EstadoReserva::EnEjecucion {
            bail!("la reserva debe estar en ejecución para publicar su acceso");
        }
        let plantilla = self.catalogo.obtener(&recibo.id_plantilla)?;
        let canal = plantilla
            .canal_acceso
            .context("la plantilla no declara un canal de acceso")?;
        let direccion = self
            .proveedor
            .direccion_instancia(&recibo.id_instancia)?
            .context("la instancia aún no publica una dirección utilizable")?;
        Ok(PuntoAcceso {
            protocolo: canal.protocolo,
            direccion,
            puerto: canal.puerto,
        })
    }

    pub fn detener(
        &self,
        id_ejecucion: &Identificador,
        confirmacion: &Identificador,
    ) -> Result<ReciboReserva> {
        exigir_confirmacion(id_ejecucion, confirmacion)?;
        let mut recibo = self.almacen.cargar(id_ejecucion)?;
        if recibo.estado != EstadoReserva::EnEjecucion {
            bail!("la reserva no está en ejecución");
        }
        match self.proveedor.estado_instancia(&recibo.id_instancia)? {
            EstadoInstancia::Encendida => {
                self.proveedor.solicitar_apagado(&recibo.id_instancia)?;
                if !self
                    .proveedor
                    .esperar_apagado(&recibo.id_instancia, PLAZO_APAGADO)?
                {
                    bail!("el sistema invitado no confirmó el apagado; no se forzó la instancia");
                }
            }
            EstadoInstancia::Apagada => {}
            EstadoInstancia::Ausente => bail!("la instancia registrada no existe"),
        }
        recibo.detener(self.reloj.ahora_unix_ms())?;
        self.almacen.actualizar(&recibo)?;
        Ok(recibo)
    }

    pub fn proteger_resultados(
        &self,
        id_ejecucion: &Identificador,
        confirmacion: &Identificador,
    ) -> Result<ReciboReserva> {
        exigir_confirmacion(id_ejecucion, confirmacion)?;
        let mut recibo = self.almacen.cargar(id_ejecucion)?;
        let resultados = self.resultados.verificar(id_ejecucion)?;
        recibo.proteger_resultados(resultados, self.reloj.ahora_unix_ms())?;
        self.almacen.actualizar(&recibo)?;
        Ok(recibo)
    }

    pub fn descartar(
        &self,
        id_ejecucion: &Identificador,
        confirmacion: &Identificador,
    ) -> Result<ReciboReserva> {
        exigir_confirmacion(id_ejecucion, confirmacion)?;
        let mut recibo = self.almacen.cargar(id_ejecucion)?;
        if recibo.estado != EstadoReserva::ResultadosProtegidos {
            bail!("los resultados no están protegidos; se rechaza el descarte");
        }
        self.exigir_instancia_apagada(&recibo)?;
        let plantilla = self.catalogo.obtener(&recibo.id_plantilla)?;
        self.proveedor
            .retirar_instancia(&plantilla, &recibo.id_instancia)?;
        recibo.descartar(self.reloj.ahora_unix_ms())?;
        self.almacen.actualizar(&recibo)?;
        Ok(recibo)
    }

    pub fn marcar_fallida(
        &self,
        id_ejecucion: &Identificador,
        motivo: MotivoFallo,
        confirmacion: &Identificador,
    ) -> Result<ReciboReserva> {
        exigir_confirmacion(id_ejecucion, confirmacion)?;
        let mut recibo = self.almacen.cargar(id_ejecucion)?;
        self.exigir_instancia_apagada(&recibo)?;
        recibo.marcar_fallida(motivo, self.reloj.ahora_unix_ms())?;
        self.almacen.actualizar(&recibo)?;
        Ok(recibo)
    }

    pub fn descartar_fallida(
        &self,
        id_ejecucion: &Identificador,
        confirmacion: &Identificador,
        acepta_perdida_resultados: bool,
    ) -> Result<ReciboReserva> {
        exigir_confirmacion(id_ejecucion, confirmacion)?;
        if !acepta_perdida_resultados {
            bail!("falta aceptar explícitamente la pérdida de resultados");
        }
        let mut recibo = self.almacen.cargar(id_ejecucion)?;
        if recibo.estado != EstadoReserva::Fallida {
            bail!("solo una reserva fallida puede descartarse sin resultados");
        }
        self.exigir_instancia_apagada(&recibo)?;
        let plantilla = self.catalogo.obtener(&recibo.id_plantilla)?;
        self.proveedor
            .retirar_instancia(&plantilla, &recibo.id_instancia)?;
        recibo.descartar_fallida(self.reloj.ahora_unix_ms())?;
        self.almacen.actualizar(&recibo)?;
        Ok(recibo)
    }

    fn exigir_instancia_apagada(&self, recibo: &ReciboReserva) -> Result<()> {
        if self.proveedor.estado_instancia(&recibo.id_instancia)? == EstadoInstancia::Encendida {
            bail!("la instancia debe estar apagada");
        }
        Ok(())
    }
}

fn exigir_confirmacion(id_ejecucion: &Identificador, confirmacion: &Identificador) -> Result<()> {
    if id_ejecucion != confirmacion {
        bail!("la confirmación no coincide exactamente con el identificador de ejecución");
    }
    Ok(())
}

#[cfg(test)]
mod pruebas {
    use super::*;
    use crate::aplicacion::puertos::GuardiaPreparacion;
    use crate::dominio::plantilla::{EstadoPlantilla, Plantilla, PoliticaRed, SistemaInvitado};
    use crate::dominio::reserva::ResultadosProtegidos;
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::Mutex;

    #[derive(Clone)]
    struct Catalogo(Plantilla);

    impl CatalogoPlantillas for Catalogo {
        fn obtener(&self, id: &Identificador) -> Result<Plantilla> {
            if &self.0.id != id {
                bail!("plantilla no registrada");
            }
            Ok(self.0.clone())
        }

        fn listar(&self) -> Result<Vec<Plantilla>> {
            Ok(vec![self.0.clone()])
        }
    }

    struct Proveedor {
        plantilla_apagada: bool,
        instancia: Mutex<EstadoInstancia>,
    }

    impl ProveedorInstancias for Proveedor {
        fn inspeccionar_plantilla(&self, _: &Plantilla) -> Result<EstadoPlantilla> {
            Ok(EstadoPlantilla {
                apagada: self.plantilla_apagada,
                sin_estado_guardado: true,
                identidad_coincide: true,
                discos_sistema: 1,
                formato_incremental_compatible: true,
                origen_registrado: true,
                red_conforme: true,
            })
        }

        fn preparar_instancia(&self, _: &Plantilla, _: &ReciboReserva) -> Result<()> {
            *self.instancia.lock().unwrap() = EstadoInstancia::Apagada;
            Ok(())
        }

        fn estado_instancia(&self, _: &Identificador) -> Result<EstadoInstancia> {
            Ok(*self.instancia.lock().unwrap())
        }

        fn iniciar_instancia(&self, _: &Identificador) -> Result<()> {
            *self.instancia.lock().unwrap() = EstadoInstancia::Encendida;
            Ok(())
        }

        fn solicitar_apagado(&self, _: &Identificador) -> Result<()> {
            *self.instancia.lock().unwrap() = EstadoInstancia::Apagada;
            Ok(())
        }

        fn esperar_apagado(&self, _: &Identificador, _: Duration) -> Result<bool> {
            Ok(*self.instancia.lock().unwrap() == EstadoInstancia::Apagada)
        }

        fn direccion_instancia(&self, _: &Identificador) -> Result<Option<std::net::IpAddr>> {
            Ok(Some("192.0.2.10".parse().unwrap()))
        }

        fn retirar_instancia(&self, _: &Plantilla, _: &Identificador) -> Result<()> {
            *self.instancia.lock().unwrap() = EstadoInstancia::Ausente;
            Ok(())
        }
    }

    #[derive(Default)]
    struct Almacen(Mutex<BTreeMap<Identificador, ReciboReserva>>);

    impl AlmacenRecibosReserva for Almacen {
        fn bloquear_preparacion(&self) -> Result<Box<dyn GuardiaPreparacion + '_>> {
            Ok(Box::new(GuardiaNula))
        }

        fn existe(&self, id: &Identificador) -> Result<bool> {
            Ok(self.0.lock().unwrap().contains_key(id))
        }

        fn guardar_nuevo(&self, recibo: &ReciboReserva) -> Result<()> {
            if self
                .0
                .lock()
                .unwrap()
                .insert(recibo.id_ejecucion.clone(), recibo.clone())
                .is_some()
            {
                bail!("duplicado");
            }
            Ok(())
        }

        fn cargar(&self, id: &Identificador) -> Result<ReciboReserva> {
            self.0
                .lock()
                .unwrap()
                .get(id)
                .cloned()
                .context("recibo ausente")
        }

        fn listar(&self) -> Result<Vec<ReciboReserva>> {
            Ok(self.0.lock().unwrap().values().cloned().collect())
        }

        fn actualizar(&self, recibo: &ReciboReserva) -> Result<()> {
            self.0
                .lock()
                .unwrap()
                .insert(recibo.id_ejecucion.clone(), recibo.clone());
            Ok(())
        }
    }

    struct GuardiaNula;

    impl GuardiaPreparacion for GuardiaNula {}

    struct Resultados;

    impl VerificadorResultados for Resultados {
        fn verificar(&self, _: &Identificador) -> Result<ResultadosProtegidos> {
            Ok(ResultadosProtegidos {
                sha256_manifiesto: "b".repeat(64),
                artefactos: 3,
                bytes: 512,
            })
        }
    }

    struct RelojFijo;

    impl Reloj for RelojFijo {
        fn ahora_unix_ms(&self) -> u64 {
            100
        }
    }

    fn plantilla() -> Plantilla {
        Plantilla {
            id: Identificador::nuevo("windows-analisis").unwrap(),
            sistema: SistemaInvitado::Windows,
            politica_red: PoliticaRed::Aislada,
            canal_acceso: Some(crate::dominio::plantilla::CanalAcceso {
                protocolo: crate::dominio::plantilla::ProtocoloAcceso::Ssh,
                puerto: 22,
            }),
            capacidades: BTreeSet::from([Identificador::nuevo("analisis-windows").unwrap()]),
        }
    }

    fn gestor(
        apagada: bool,
    ) -> GestorReservas<Catalogo, Proveedor, Almacen, Resultados, RelojFijo> {
        gestor_con_limite(apagada, 4)
    }

    fn gestor_con_limite(
        apagada: bool,
        limite: usize,
    ) -> GestorReservas<Catalogo, Proveedor, Almacen, Resultados, RelojFijo> {
        GestorReservas::nuevo(
            Catalogo(plantilla()),
            Proveedor {
                plantilla_apagada: apagada,
                instancia: Mutex::new(EstadoInstancia::Ausente),
            },
            Almacen::default(),
            Resultados,
            RelojFijo,
            limite,
        )
        .unwrap()
    }

    #[test]
    fn no_prepara_desde_una_plantilla_encendida() {
        let id = Identificador::nuevo("ejecucion-001").unwrap();
        assert!(gestor(false)
            .preparar(plantilla().id, id.clone(), &id)
            .is_err());
    }

    #[test]
    fn recorre_el_ciclo_con_resultados_verificados() {
        let id = Identificador::nuevo("ejecucion-002").unwrap();
        let gestor = gestor(true);
        assert_eq!(
            gestor
                .preparar(plantilla().id, id.clone(), &id)
                .unwrap()
                .estado,
            EstadoReserva::Preparada
        );
        assert_eq!(
            gestor.iniciar(&id, &id).unwrap().estado,
            EstadoReserva::EnEjecucion
        );
        assert_eq!(
            gestor.detener(&id, &id).unwrap().estado,
            EstadoReserva::Detenida
        );
        assert_eq!(
            gestor.proteger_resultados(&id, &id).unwrap().estado,
            EstadoReserva::ResultadosProtegidos
        );
        assert_eq!(
            gestor.descartar(&id, &id).unwrap().estado,
            EstadoReserva::Descartada
        );
    }

    #[test]
    fn reconcilia_un_arranque_persistido_a_medias() {
        let id = Identificador::nuevo("ejecucion-003").unwrap();
        let gestor = gestor(true);
        gestor.preparar(plantilla().id, id.clone(), &id).unwrap();
        gestor
            .proveedor
            .iniciar_instancia(&Identificador::nuevo("lab-ejecucion-003").unwrap())
            .unwrap();
        let reconciliada = gestor.reconciliar(&id, &id).unwrap();
        assert_eq!(reconciliada.estado, EstadoReserva::EnEjecucion);
    }

    #[test]
    fn publica_el_acceso_solo_durante_la_ejecucion() {
        let id = Identificador::nuevo("ejecucion-004").unwrap();
        let gestor = gestor(true);
        gestor.preparar(plantilla().id, id.clone(), &id).unwrap();
        assert!(gestor.obtener_acceso(&id).is_err());
        gestor.iniciar(&id, &id).unwrap();
        let acceso = gestor.obtener_acceso(&id).unwrap();
        assert_eq!(acceso.puerto, 22);
    }

    #[test]
    fn impide_superar_la_cuota_de_reservas_activas() {
        let primera = Identificador::nuevo("ejecucion-005").unwrap();
        let segunda = Identificador::nuevo("ejecucion-006").unwrap();
        let gestor = gestor_con_limite(true, 1);
        gestor
            .preparar(plantilla().id.clone(), primera.clone(), &primera)
            .unwrap();
        assert!(gestor
            .preparar(plantilla().id, segunda.clone(), &segunda)
            .is_err());
    }
}
