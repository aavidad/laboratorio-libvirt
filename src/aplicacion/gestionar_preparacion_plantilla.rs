// Laboratorio de máquinas virtuales con libvirt
// Copyright (C) 2026 Alberto Avidad
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::aplicacion::gestionar_reserva::PLAZO_APAGADO;
use crate::aplicacion::puertos::{
    AlmacenRecibosReserva, CatalogoDestinosPromocion, EstadoInstancia, ObservadorArranque,
    ProveedorInstancias, ProveedorPreparacionPlantillas, Reloj,
};
use crate::dominio::arranque::diagnosticar_arranque;
use crate::dominio::identificador::Identificador;
use crate::dominio::plantilla::ProtocoloAcceso;
use crate::dominio::preparacion_plantilla::{
    diagnosticar_candidata, ComprobacionIdentidadSsh, DestinoPromocion, FasePreparacionPlantilla,
};
use crate::dominio::reserva::{EstadoReserva, ReciboReserva};
use anyhow::{bail, Result};

/// Flujo explícito para sanear y aceptar una nueva plantilla sin modificar la
/// anterior. Los recursos concretos solo los conoce el proveedor.
pub struct GestorPreparacionPlantillas<C, P, A, R> {
    catalogo: C,
    proveedor: P,
    almacen: A,
    reloj: R,
}

impl<C, P, A, R> GestorPreparacionPlantillas<C, P, A, R>
where
    C: CatalogoDestinosPromocion,
    P: ProveedorInstancias + ProveedorPreparacionPlantillas + ObservadorArranque,
    A: AlmacenRecibosReserva,
    R: Reloj,
{
    pub fn nuevo(catalogo: C, proveedor: P, almacen: A, reloj: R) -> Self {
        Self {
            catalogo,
            proveedor,
            almacen,
            reloj,
        }
    }

    pub fn sanear(
        &self,
        id_ejecucion: &Identificador,
        id_destino: &Identificador,
        confirmacion: &Identificador,
    ) -> Result<ReciboReserva> {
        exigir_confirmacion(id_ejecucion, confirmacion)?;
        let _bloqueo = self.almacen.bloquear_mutacion(id_ejecucion)?;
        let mut recibo = self.almacen.cargar(id_ejecucion)?;
        let destino = self.destino_admitido(&recibo, id_destino)?;
        if let Some(progreso) = &recibo.preparacion_plantilla {
            if progreso.id_plantilla_destino != *id_destino {
                bail!("la reserva ya está ligada a otro destino de promoción");
            }
            return Ok(recibo);
        }
        self.exigir_apagada(&recibo)?;
        self.proveedor.sanear_candidata(&recibo, &destino)?;
        let diagnostico =
            diagnosticar_candidata(self.proveedor.inspeccionar_candidata(&recibo, &destino)?);
        if !diagnostico.preparada {
            bail!("el saneamiento no dejó la candidata en un estado verificable");
        }
        recibo.registrar_saneamiento(id_destino.clone(), self.reloj.ahora_unix_ms())?;
        self.almacen.actualizar(&recibo)?;
        Ok(recibo)
    }

    pub fn iniciar_ciclo(
        &self,
        id_ejecucion: &Identificador,
        confirmacion: &Identificador,
    ) -> Result<ReciboReserva> {
        exigir_confirmacion(id_ejecucion, confirmacion)?;
        let _bloqueo = self.almacen.bloquear_mutacion(id_ejecucion)?;
        let mut recibo = self.almacen.cargar(id_ejecucion)?;
        let progreso = recibo
            .preparacion_plantilla
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("la candidata aún no ha sido saneada"))?;
        let fase = progreso.fase;
        let id_destino = progreso.id_plantilla_destino.clone();
        let destino = self.destino_admitido(&recibo, &id_destino)?;
        let estado_instancia = self.proveedor.estado_instancia(&recibo.id_instancia)?;
        if matches!(
            fase,
            FasePreparacionPlantilla::CicloUnoEnCurso | FasePreparacionPlantilla::CicloDosEnCurso
        ) && recibo.estado == EstadoReserva::EnEjecucion
            && estado_instancia == EstadoInstancia::Encendida
        {
            return Ok(recibo);
        }
        if recibo.estado != EstadoReserva::Detenida
            || !matches!(
                fase,
                FasePreparacionPlantilla::Saneada | FasePreparacionPlantilla::CicloUnoValidado
            )
            || estado_instancia == EstadoInstancia::Ausente
        {
            bail!("el estado no permite iniciar el siguiente ciclo");
        }
        let diagnostico =
            diagnosticar_candidata(self.proveedor.inspeccionar_candidata(&recibo, &destino)?);
        if estado_instancia == EstadoInstancia::Apagada && !diagnostico.preparada {
            bail!("la candidata dejó de cumplir las invariantes antes del arranque");
        }
        if estado_instancia == EstadoInstancia::Apagada {
            let diagnostico =
                diagnosticar_arranque(true, self.proveedor.observar_arranque(&recibo)?);
            if !diagnostico.iniciable {
                let fallos = diagnostico
                    .comprobaciones
                    .iter()
                    .filter(|comprobacion| !comprobacion.correcta)
                    .map(|comprobacion| comprobacion.codigo.codigo())
                    .collect::<Vec<_>>()
                    .join(", ");
                bail!("la candidata no cumple las precondiciones de arranque: {fallos}");
            }
            self.proveedor.iniciar_instancia(&recibo.id_instancia)?;
        }
        recibo.iniciar_ciclo_plantilla(self.reloj.ahora_unix_ms())?;
        self.almacen.actualizar(&recibo)?;
        Ok(recibo)
    }

    pub fn detener_ciclo(
        &self,
        id_ejecucion: &Identificador,
        confirmacion: &Identificador,
    ) -> Result<ReciboReserva> {
        exigir_confirmacion(id_ejecucion, confirmacion)?;
        let _bloqueo = self.almacen.bloquear_mutacion(id_ejecucion)?;
        let mut recibo = self.almacen.cargar(id_ejecucion)?;
        if recibo.estado != EstadoReserva::EnEjecucion
            || !matches!(
                recibo
                    .preparacion_plantilla
                    .as_ref()
                    .map(|valor| valor.fase),
                Some(
                    FasePreparacionPlantilla::CicloUnoEnCurso
                        | FasePreparacionPlantilla::CicloDosEnCurso
                )
            )
        {
            bail!("no hay un ciclo de preparación en ejecución");
        }
        let id_destino = recibo
            .preparacion_plantilla
            .as_ref()
            .expect("se comprobó el progreso")
            .id_plantilla_destino
            .clone();
        let destino = self.destino_admitido(&recibo, &id_destino)?;
        let exige_identidad_ssh = destino
            .plantilla
            .canal_acceso
            .as_ref()
            .is_some_and(|canal| canal.protocolo == ProtocoloAcceso::Ssh);
        let estado_instancia = self.proveedor.estado_instancia(&recibo.id_instancia)?;
        let identidad_observada =
            if estado_instancia == EstadoInstancia::Encendida && exige_identidad_ssh {
                Some(
                    self.proveedor
                        .validar_identidad_ssh_candidata(&recibo, &destino)?,
                )
            } else {
                None
            };
        if let Some(identidad) = &identidad_observada {
            if let Some(anterior) = recibo
                .preparacion_plantilla
                .as_ref()
                .and_then(|progreso| progreso.ciclos_validados.last())
                .map(|ciclo| &ciclo.identidad_ssh)
            {
                if !anterior.aplicable
                    || anterior.huella_sha256.as_deref() != Some(identidad.huella_sha256.as_str())
                {
                    bail!("la clave de host SSH cambió entre ciclos de aceptación");
                }
            }
            if let Some(en_curso) = recibo
                .preparacion_plantilla
                .as_ref()
                .and_then(|progreso| progreso.identidad_ssh_en_curso.as_ref())
            {
                if en_curso.huella_sha256.as_deref() != Some(identidad.huella_sha256.as_str()) {
                    bail!("la clave de host SSH cambió durante el ciclo de aceptación");
                }
            }
        }
        let identidad_ya_registrada = recibo
            .preparacion_plantilla
            .as_ref()
            .and_then(|progreso| progreso.identidad_ssh_en_curso.as_ref())
            .is_some_and(ComprobacionIdentidadSsh::es_aceptable);
        if !identidad_ya_registrada {
            if estado_instancia != EstadoInstancia::Encendida && exige_identidad_ssh {
                bail!("la candidata se apagó antes de validar su identidad SSH por QGA");
            }
            let instante = self.reloj.ahora_unix_ms();
            let comprobacion = if exige_identidad_ssh {
                let identidad = identidad_observada
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("no se observó la identidad SSH por QGA"))?;
                ComprobacionIdentidadSsh::validada(instante, identidad.huella_sha256.clone())
            } else {
                ComprobacionIdentidadSsh::no_aplicable()
            };
            recibo.registrar_identidad_ssh_ciclo(comprobacion, instante)?;
            self.almacen.actualizar(&recibo)?;
        }
        match estado_instancia {
            EstadoInstancia::Encendida => {
                self.proveedor.solicitar_apagado(&recibo.id_instancia)?;
                if !self
                    .proveedor
                    .esperar_apagado(&recibo.id_instancia, PLAZO_APAGADO)?
                {
                    bail!("el invitado no confirmó el apagado; no se forzó la candidata");
                }
            }
            EstadoInstancia::Apagada => {}
            EstadoInstancia::Ausente => bail!("la candidata registrada no existe"),
        }
        recibo.detener(self.reloj.ahora_unix_ms())?;
        self.almacen.actualizar(&recibo)?;
        Ok(recibo)
    }

    pub fn validar_ciclo(
        &self,
        id_ejecucion: &Identificador,
        confirmacion: &Identificador,
    ) -> Result<ReciboReserva> {
        exigir_confirmacion(id_ejecucion, confirmacion)?;
        let _bloqueo = self.almacen.bloquear_mutacion(id_ejecucion)?;
        let mut recibo = self.almacen.cargar(id_ejecucion)?;
        let progreso = recibo
            .preparacion_plantilla
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("la candidata aún no ha sido saneada"))?;
        let destino = self.destino_admitido(&recibo, &progreso.id_plantilla_destino)?;
        self.exigir_apagada(&recibo)?;
        let diagnostico =
            diagnosticar_candidata(self.proveedor.inspeccionar_candidata(&recibo, &destino)?);
        recibo.registrar_validacion_ciclo(diagnostico, self.reloj.ahora_unix_ms())?;
        self.almacen.actualizar(&recibo)?;
        Ok(recibo)
    }

    pub fn promover(
        &self,
        id_ejecucion: &Identificador,
        confirmacion: &Identificador,
    ) -> Result<ReciboReserva> {
        exigir_confirmacion(id_ejecucion, confirmacion)?;
        let _bloqueo = self.almacen.bloquear_mutacion(id_ejecucion)?;
        let mut recibo = self.almacen.cargar(id_ejecucion)?;
        if recibo.estado == EstadoReserva::Promovida {
            return Ok(recibo);
        }
        let fase = recibo
            .preparacion_plantilla
            .as_ref()
            .map(|progreso| progreso.fase)
            .ok_or_else(|| anyhow::anyhow!("la candidata aún no ha sido saneada"))?;
        let id_destino = recibo
            .preparacion_plantilla
            .as_ref()
            .expect("se comprobó el progreso")
            .id_plantilla_destino
            .clone();
        let destino = self.destino_admitido(&recibo, &id_destino)?;
        match fase {
            FasePreparacionPlantilla::CicloDosValidado => {
                self.exigir_apagada(&recibo)?;
                let diagnostico = diagnosticar_candidata(
                    self.proveedor.inspeccionar_candidata(&recibo, &destino)?,
                );
                if !diagnostico.preparada {
                    bail!("la candidata dejó de cumplir las invariantes antes de promoverla");
                }
                self.proveedor.comprobar_destino_libre(&destino)?;
                recibo.iniciar_promocion(self.reloj.ahora_unix_ms())?;
                self.almacen.actualizar(&recibo)?;
            }
            FasePreparacionPlantilla::PromocionEnCurso => {}
            _ => bail!("faltan los dos ciclos de arranque, apagado y validación"),
        }
        self.proveedor.promover_candidata(&recibo, &destino)?;
        recibo.marcar_promovida(self.reloj.ahora_unix_ms())?;
        self.almacen.actualizar(&recibo)?;
        Ok(recibo)
    }

    fn destino_admitido(
        &self,
        recibo: &ReciboReserva,
        id_destino: &Identificador,
    ) -> Result<DestinoPromocion> {
        let destino = self.catalogo.obtener_destino(id_destino)?;
        if destino.id_origen != recibo.id_plantilla || destino.plantilla.sistema != recibo.sistema {
            bail!("el destino no admite como origen la plantilla de la reserva");
        }
        Ok(destino)
    }

    fn exigir_apagada(&self, recibo: &ReciboReserva) -> Result<()> {
        if self.proveedor.estado_instancia(&recibo.id_instancia)? != EstadoInstancia::Apagada {
            bail!("la candidata debe existir y estar apagada");
        }
        Ok(())
    }
}

fn exigir_confirmacion(id: &Identificador, confirmacion: &Identificador) -> Result<()> {
    if id != confirmacion {
        bail!("la confirmación no coincide exactamente con el identificador de ejecución");
    }
    Ok(())
}

#[cfg(test)]
mod pruebas {
    use super::*;
    use crate::aplicacion::puertos::{
        GuardiaMutacion, GuardiaPreparacion, ProveedorPreparacionPlantillas,
    };
    use crate::dominio::plantilla::{
        CanalAcceso, EstadoPlantilla, IdentidadServidor, Plantilla, PoliticaRed, ProtocoloAcceso,
        SistemaInvitado,
    };
    use crate::dominio::preparacion_plantilla::{DestinoPromocion, EstadoCandidataPlantilla};
    use std::collections::{BTreeMap, BTreeSet};
    use std::net::IpAddr;
    use std::sync::Mutex;
    use std::time::Duration;

    #[derive(Clone)]
    struct Catalogo(DestinoPromocion);

    impl CatalogoDestinosPromocion for Catalogo {
        fn obtener_destino(&self, id: &Identificador) -> Result<DestinoPromocion> {
            if self.0.plantilla.id != *id {
                bail!("destino no registrado");
            }
            Ok(self.0.clone())
        }

        fn listar_destinos(&self) -> Result<Vec<DestinoPromocion>> {
            Ok(vec![self.0.clone()])
        }
    }

    struct Proveedor {
        estado: Mutex<EstadoInstancia>,
        saneamientos: Mutex<u8>,
        promociones: Mutex<u8>,
        fallar_promocion: Mutex<bool>,
        conforme: Mutex<bool>,
        identidad_ssh_valida: Mutex<bool>,
        identidades_ssh_validadas: Mutex<u8>,
        huella_ssh: Mutex<String>,
    }

    impl ProveedorInstancias for Proveedor {
        fn inspeccionar_plantilla(&self, _: &Plantilla) -> Result<EstadoPlantilla> {
            unreachable!()
        }

        fn preparar_instancia(&self, _: &Plantilla, _: &ReciboReserva) -> Result<()> {
            unreachable!()
        }

        fn estado_instancia(&self, _: &Identificador) -> Result<EstadoInstancia> {
            Ok(*self.estado.lock().unwrap())
        }

        fn iniciar_instancia(&self, _: &Identificador) -> Result<()> {
            *self.estado.lock().unwrap() = EstadoInstancia::Encendida;
            Ok(())
        }

        fn solicitar_apagado(&self, _: &Identificador) -> Result<()> {
            *self.estado.lock().unwrap() = EstadoInstancia::Apagada;
            Ok(())
        }

        fn esperar_apagado(&self, _: &Identificador, _: Duration) -> Result<bool> {
            Ok(*self.estado.lock().unwrap() == EstadoInstancia::Apagada)
        }

        fn direccion_instancia(&self, _: &Identificador) -> Result<Option<IpAddr>> {
            unreachable!()
        }

        fn identidad_servidor(
            &self,
            _: &Plantilla,
            _: &ReciboReserva,
        ) -> Result<Option<IdentidadServidor>> {
            unreachable!()
        }

        fn retirar_instancia(&self, _: &Plantilla, _: &Identificador) -> Result<()> {
            unreachable!()
        }
    }

    impl ObservadorArranque for Proveedor {
        fn observar_arranque(
            &self,
            _: &ReciboReserva,
        ) -> Result<crate::dominio::arranque::EstadoArranqueObservado> {
            let estado = *self.estado.lock().unwrap();
            Ok(crate::dominio::arranque::EstadoArranqueObservado {
                instancia_registrada: estado != EstadoInstancia::Ausente,
                instancia_apagada: estado == EstadoInstancia::Apagada,
                definicion_valida: true,
                almacenamiento_presente: true,
                recursos_escritura_disponibles: true,
                redes_requeridas_activas: true,
                estado_guardado_ausente: true,
                ultimo_estado_fallido: false,
                ultimo_fallo: None,
            })
        }
    }

    impl ProveedorPreparacionPlantillas for Proveedor {
        fn comprobar_destino_libre(&self, _: &DestinoPromocion) -> Result<()> {
            Ok(())
        }

        fn sanear_candidata(&self, _: &ReciboReserva, _: &DestinoPromocion) -> Result<()> {
            *self.saneamientos.lock().unwrap() += 1;
            *self.conforme.lock().unwrap() = true;
            Ok(())
        }

        fn inspeccionar_candidata(
            &self,
            _: &ReciboReserva,
            _: &DestinoPromocion,
        ) -> Result<EstadoCandidataPlantilla> {
            let conforme = *self.conforme.lock().unwrap();
            Ok(EstadoCandidataPlantilla {
                apagada: *self.estado.lock().unwrap() == EstadoInstancia::Apagada,
                sin_medios_temporales: conforme,
                red_final_conforme: conforme,
                disco_sistema_unico: true,
                identidad_independiente: true,
            })
        }

        fn validar_identidad_ssh_candidata(
            &self,
            _: &ReciboReserva,
            _: &DestinoPromocion,
        ) -> Result<IdentidadServidor> {
            if !*self.identidad_ssh_valida.lock().unwrap() {
                bail!("identidad SSH simulada no válida");
            }
            *self.identidades_ssh_validadas.lock().unwrap() += 1;
            Ok(IdentidadServidor {
                algoritmo: Identificador::nuevo("ssh-ed25519").unwrap(),
                clave_publica: concat!(
                    "ssh-ed25519 ",
                    "AAAAC3NzaC1lZDI1NTE5AAAAIFhmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZm"
                )
                .to_owned(),
                huella_sha256: self.huella_ssh.lock().unwrap().clone(),
            })
        }

        fn promover_candidata(&self, _: &ReciboReserva, _: &DestinoPromocion) -> Result<()> {
            if *self.fallar_promocion.lock().unwrap() {
                bail!("fallo simulado");
            }
            *self.promociones.lock().unwrap() += 1;
            Ok(())
        }
    }

    #[derive(Default)]
    struct Almacen(Mutex<BTreeMap<Identificador, ReciboReserva>>);

    impl AlmacenRecibosReserva for Almacen {
        fn bloquear_preparacion(&self) -> Result<Box<dyn GuardiaPreparacion + '_>> {
            Ok(Box::new(GuardiaNula))
        }

        fn bloquear_mutacion(&self, _: &Identificador) -> Result<Box<dyn GuardiaMutacion + '_>> {
            Ok(Box::new(GuardiaNula))
        }

        fn existe(&self, id: &Identificador) -> Result<bool> {
            Ok(self.0.lock().unwrap().contains_key(id))
        }

        fn guardar_nuevo(&self, recibo: &ReciboReserva) -> Result<()> {
            self.0
                .lock()
                .unwrap()
                .insert(recibo.id_ejecucion.clone(), recibo.clone());
            Ok(())
        }

        fn cargar(&self, id: &Identificador) -> Result<ReciboReserva> {
            self.0
                .lock()
                .unwrap()
                .get(id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("recibo ausente"))
        }

        fn listar(&self) -> Result<Vec<ReciboReserva>> {
            Ok(self.0.lock().unwrap().values().cloned().collect())
        }

        fn actualizar(&self, recibo: &ReciboReserva) -> Result<()> {
            let mut recibos = self.0.lock().unwrap();
            let anterior = recibos
                .get(&recibo.id_ejecucion)
                .ok_or_else(|| anyhow::anyhow!("recibo ausente"))?;
            if anterior.revision.checked_add(1) != Some(recibo.revision) {
                bail!("conflicto de revisión");
            }
            recibos.insert(recibo.id_ejecucion.clone(), recibo.clone());
            Ok(())
        }
    }

    struct GuardiaNula;
    impl GuardiaPreparacion for GuardiaNula {}
    impl GuardiaMutacion for GuardiaNula {}

    struct RelojFijo;
    impl Reloj for RelojFijo {
        fn ahora_unix_ms(&self) -> u64 {
            100
        }
    }

    fn ids() -> (Identificador, Identificador, Identificador) {
        (
            Identificador::nuevo("campana-plantilla").unwrap(),
            Identificador::nuevo("windows-origen").unwrap(),
            Identificador::nuevo("windows-nueva").unwrap(),
        )
    }

    fn gestor() -> GestorPreparacionPlantillas<Catalogo, Proveedor, Almacen, RelojFijo> {
        let (id_ejecucion, id_origen, id_destino) = ids();
        let mut recibo = ReciboReserva::nuevo(
            id_ejecucion.clone(),
            id_origen.clone(),
            SistemaInvitado::Windows,
            1,
        )
        .unwrap();
        recibo.iniciar(2).unwrap();
        recibo.detener(3).unwrap();
        let almacen = Almacen::default();
        almacen.guardar_nuevo(&recibo).unwrap();
        let destino = DestinoPromocion {
            plantilla: Plantilla {
                id: id_destino,
                sistema: SistemaInvitado::Windows,
                politica_red: PoliticaRed::Aislada,
                canal_acceso: Some(CanalAcceso {
                    protocolo: ProtocoloAcceso::Ssh,
                    puerto: 22,
                }),
                capacidades: BTreeSet::new(),
            },
            id_origen,
        };
        GestorPreparacionPlantillas::nuevo(
            Catalogo(destino),
            Proveedor {
                estado: Mutex::new(EstadoInstancia::Apagada),
                saneamientos: Mutex::new(0),
                promociones: Mutex::new(0),
                fallar_promocion: Mutex::new(false),
                conforme: Mutex::new(false),
                identidad_ssh_valida: Mutex::new(true),
                identidades_ssh_validadas: Mutex::new(0),
                huella_ssh: Mutex::new(
                    "SHA256:m/ejOXkI0ofKT+bW34mhHdSH63xu9c+QU7djuKirDXM".to_owned(),
                ),
            },
            almacen,
            RelojFijo,
        )
    }

    #[test]
    fn exige_dos_ciclos_observables_antes_de_promover() {
        let (id, _, destino) = ids();
        let gestor = gestor();
        gestor.sanear(&id, &destino, &id).unwrap();
        assert!(gestor.promover(&id, &id).is_err());
        for _ in 0..2 {
            gestor.iniciar_ciclo(&id, &id).unwrap();
            gestor.detener_ciclo(&id, &id).unwrap();
            gestor.validar_ciclo(&id, &id).unwrap();
        }
        let recibo = gestor.promover(&id, &id).unwrap();
        assert_eq!(recibo.estado, EstadoReserva::Promovida);
        let progreso = recibo.preparacion_plantilla.unwrap();
        assert_eq!(progreso.ciclos_validados.len(), 2);
        assert!(progreso.ciclos_validados.iter().all(|ciclo| {
            ciclo.identidad_ssh.aplicable
                && ciclo.identidad_ssh.correcta
                && ciclo.identidad_ssh.observada_en_unix_ms.is_some()
        }));
        assert_eq!(
            *gestor.proveedor.identidades_ssh_validadas.lock().unwrap(),
            2
        );
    }

    #[test]
    fn no_apaga_ni_acepta_un_ciclo_ssh_sin_identidad_qga_valida() {
        let (id, _, destino) = ids();
        let gestor = gestor();
        gestor.sanear(&id, &destino, &id).unwrap();
        gestor.iniciar_ciclo(&id, &id).unwrap();
        *gestor.proveedor.identidad_ssh_valida.lock().unwrap() = false;
        assert!(gestor.detener_ciclo(&id, &id).is_err());
        assert_eq!(
            *gestor.proveedor.estado.lock().unwrap(),
            EstadoInstancia::Encendida
        );
        let recibo = gestor.almacen.cargar(&id).unwrap();
        assert_eq!(recibo.estado, EstadoReserva::EnEjecucion);
        assert!(recibo
            .preparacion_plantilla
            .unwrap()
            .identidad_ssh_en_curso
            .is_none());
    }

    #[test]
    fn registra_identidad_ssh_no_aplicable_en_un_destino_sin_ssh() {
        let (id, _, destino) = ids();
        let mut gestor = gestor();
        gestor.catalogo.0.plantilla.canal_acceso = None;
        gestor.sanear(&id, &destino, &id).unwrap();
        gestor.iniciar_ciclo(&id, &id).unwrap();
        gestor.detener_ciclo(&id, &id).unwrap();
        let recibo = gestor.validar_ciclo(&id, &id).unwrap();
        let progreso = recibo.preparacion_plantilla.unwrap();
        let identidad = &progreso.ciclos_validados[0].identidad_ssh;
        assert!(!identidad.aplicable);
        assert!(!identidad.correcta);
        assert!(identidad.observada_en_unix_ms.is_none());
        assert_eq!(
            *gestor.proveedor.identidades_ssh_validadas.lock().unwrap(),
            0
        );
    }

    #[test]
    fn rechaza_un_cambio_de_clave_host_entre_los_dos_ciclos() {
        let (id, _, destino) = ids();
        let gestor = gestor();
        gestor.sanear(&id, &destino, &id).unwrap();
        gestor.iniciar_ciclo(&id, &id).unwrap();
        gestor.detener_ciclo(&id, &id).unwrap();
        gestor.validar_ciclo(&id, &id).unwrap();
        gestor.iniciar_ciclo(&id, &id).unwrap();
        *gestor.proveedor.huella_ssh.lock().unwrap() =
            "SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_owned();
        assert!(gestor.detener_ciclo(&id, &id).is_err());
        assert_eq!(
            *gestor.proveedor.estado.lock().unwrap(),
            EstadoInstancia::Encendida
        );
        assert!(gestor
            .almacen
            .cargar(&id)
            .unwrap()
            .preparacion_plantilla
            .unwrap()
            .identidad_ssh_en_curso
            .is_none());
    }

    #[test]
    fn saneamiento_y_promocion_son_idempotentes_por_recibo() {
        let (id, _, destino) = ids();
        let gestor = gestor();
        let primero = gestor.sanear(&id, &destino, &id).unwrap();
        let segundo = gestor.sanear(&id, &destino, &id).unwrap();
        assert_eq!(primero.revision, segundo.revision);
        assert_eq!(*gestor.proveedor.saneamientos.lock().unwrap(), 1);
        for _ in 0..2 {
            gestor.iniciar_ciclo(&id, &id).unwrap();
            gestor.detener_ciclo(&id, &id).unwrap();
            gestor.validar_ciclo(&id, &id).unwrap();
        }
        gestor.promover(&id, &id).unwrap();
        gestor.promover(&id, &id).unwrap();
        assert_eq!(*gestor.proveedor.promociones.lock().unwrap(), 1);
    }

    #[test]
    fn rechaza_confirmacion_y_destino_ajenos() {
        let (id, _, destino) = ids();
        let gestor = gestor();
        let otro = Identificador::nuevo("otro").unwrap();
        assert!(gestor.sanear(&id, &destino, &otro).is_err());
        assert!(gestor.sanear(&id, &otro, &id).is_err());
    }

    #[test]
    fn reanuda_una_promocion_interrumpida_desde_su_recibo() {
        let (id, _, destino) = ids();
        let gestor = gestor();
        gestor.sanear(&id, &destino, &id).unwrap();
        for _ in 0..2 {
            gestor.iniciar_ciclo(&id, &id).unwrap();
            gestor.detener_ciclo(&id, &id).unwrap();
            gestor.validar_ciclo(&id, &id).unwrap();
        }
        *gestor.proveedor.fallar_promocion.lock().unwrap() = true;
        assert!(gestor.promover(&id, &id).is_err());
        assert_eq!(
            gestor
                .almacen
                .cargar(&id)
                .unwrap()
                .preparacion_plantilla
                .unwrap()
                .fase,
            FasePreparacionPlantilla::PromocionEnCurso
        );
        *gestor.proveedor.fallar_promocion.lock().unwrap() = false;
        assert_eq!(
            gestor.promover(&id, &id).unwrap().estado,
            EstadoReserva::Promovida
        );
    }

    #[test]
    fn reconcilia_un_arranque_de_ciclo_persistido_a_medias() {
        let (id, _, destino) = ids();
        let gestor = gestor();
        gestor.sanear(&id, &destino, &id).unwrap();
        *gestor.proveedor.estado.lock().unwrap() = EstadoInstancia::Encendida;
        let recibo = gestor.iniciar_ciclo(&id, &id).unwrap();
        assert_eq!(recibo.estado, EstadoReserva::EnEjecucion);
        assert_eq!(
            recibo.preparacion_plantilla.unwrap().fase,
            FasePreparacionPlantilla::CicloUnoEnCurso
        );
    }
}
