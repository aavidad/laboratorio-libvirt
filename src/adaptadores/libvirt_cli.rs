// Laboratorio de máquinas virtuales con libvirt
// Copyright (C) 2026 Alberto Avidad
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::adaptadores::configuracion_json::{
    DefinicionPlantillaLibvirt, DefinicionPromocionLibvirt, PerfilIdentidadAcceso,
};
use crate::aplicacion::puertos::{
    EstadoInstancia, ObservadorAcceso, ObservadorArranque, ProveedorInstancias,
    ProveedorPreparacionPlantillas,
};
use crate::dominio::acceso::EstadoAccesoObservado;
use crate::dominio::arranque::{CodigoFalloArranque, EstadoArranqueObservado};
use crate::dominio::identificador::Identificador;
use crate::dominio::plantilla::{
    EstadoPlantilla, IdentidadServidor, Plantilla, PoliticaRed, ProtocoloAcceso,
};
use crate::dominio::preparacion_plantilla::{
    DestinoPromocion, EstadoCandidataPlantilla, FasePreparacionPlantilla,
};
use crate::dominio::reserva::ReciboReserva;
use anyhow::{bail, Context, Result};
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use base64::Engine;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::IpAddr;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};
use wait_timeout::ChildExt;
use xmltree::{Element, XMLNode};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

const VIRSH: &str = "/usr/bin/virsh";
const QEMU_IMG: &str = "/usr/bin/qemu-img";
const JOURNALCTL: &str = "/usr/bin/journalctl";
const PLAZO_COMANDO: Duration = Duration::from_secs(30);
const PLAZO_PROMOCION: Duration = Duration::from_secs(30 * 60);
const MAXIMO_BYTES_SALIDA: u64 = 16 * 1024 * 1024;
const MAXIMO_BYTES_CLAVE_PUBLICA_SSH: usize = 512;
const MAXIMO_BYTES_MARCA_UUID: usize = 128;
static CONTADOR_XML_TEMPORAL: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
struct SalidaComando {
    correcta: bool,
    salida: String,
    error: String,
}

trait EjecutorComandos: Send + Sync {
    fn ejecutar(
        &self,
        programa: &Path,
        argumentos: &[OsString],
        plazo: Duration,
    ) -> Result<SalidaComando>;
}

struct EjecutorSistema;

impl EjecutorComandos for EjecutorSistema {
    fn ejecutar(
        &self,
        programa: &Path,
        argumentos: &[OsString],
        plazo: Duration,
    ) -> Result<SalidaComando> {
        let mut hijo = Command::new(programa)
            .args(argumentos)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("no se pudo ejecutar {}", programa.display()))?;
        let salida_hijo = hijo.stdout.take().context("no se pudo capturar stdout")?;
        let error_hijo = hijo.stderr.take().context("no se pudo capturar stderr")?;
        let hilo_salida = thread::spawn(move || leer_salida(salida_hijo));
        let hilo_error = thread::spawn(move || leer_salida(error_hijo));
        let estado = match hijo.wait_timeout(plazo)? {
            Some(estado) => estado,
            None => {
                let _ = hijo.kill();
                let _ = hijo.wait();
                let _ = hilo_salida.join();
                let _ = hilo_error.join();
                bail!("libvirt no respondió dentro del plazo máximo");
            }
        };
        let stdout = hilo_salida
            .join()
            .map_err(|_| anyhow::anyhow!("falló la lectura de stdout"))??;
        let stderr = hilo_error
            .join()
            .map_err(|_| anyhow::anyhow!("falló la lectura de stderr"))??;
        Ok(SalidaComando {
            correcta: estado.success(),
            salida: String::from_utf8(stdout).context("la salida de libvirt no es UTF-8")?,
            error: String::from_utf8(stderr).context("el error de libvirt no es UTF-8")?,
        })
    }
}

fn leer_salida(lector: impl Read) -> Result<Vec<u8>> {
    let mut salida = Vec::new();
    let mut limitado = lector.take(MAXIMO_BYTES_SALIDA + 1);
    limitado.read_to_end(&mut salida)?;
    if salida.len() as u64 > MAXIMO_BYTES_SALIDA {
        bail!("la salida de libvirt supera la cuota");
    }
    Ok(salida)
}

/// Adaptador de libvirt mediante argumentos directos a `virsh`. Nunca emplea
/// un intérprete de órdenes y solo resuelve plantillas del inventario local.
pub struct LibvirtCli {
    uri: String,
    definiciones: BTreeMap<Identificador, DefinicionPlantillaLibvirt>,
    promociones: BTreeMap<Identificador, DefinicionPromocionLibvirt>,
    ejecutor: Box<dyn EjecutorComandos>,
}

impl LibvirtCli {
    pub fn nuevo(
        uri: impl Into<String>,
        definiciones: BTreeMap<Identificador, DefinicionPlantillaLibvirt>,
        promociones: BTreeMap<Identificador, DefinicionPromocionLibvirt>,
    ) -> Result<Self> {
        let uri = uri.into();
        if uri != "qemu:///system" {
            bail!("solo se admite qemu:///system");
        }
        if definiciones.is_empty() {
            bail!("el inventario libvirt está vacío");
        }
        let metadatos = fs::symlink_metadata(VIRSH).context("no se encontró /usr/bin/virsh")?;
        if metadatos.file_type().is_symlink() || !metadatos.is_file() {
            bail!("/usr/bin/virsh no es un ejecutable ordinario");
        }
        Ok(Self {
            uri,
            definiciones,
            promociones,
            ejecutor: Box::new(EjecutorSistema),
        })
    }

    fn definicion(&self, plantilla: &Plantilla) -> Result<&DefinicionPlantillaLibvirt> {
        self.definiciones
            .get(&plantilla.id)
            .context("la plantilla no tiene una definición libvirt registrada")
    }

    fn definicion_promocion(
        &self,
        destino: &DestinoPromocion,
    ) -> Result<&DefinicionPromocionLibvirt> {
        let definicion = self
            .promociones
            .get(&destino.plantilla.id)
            .context("el destino no tiene una definición libvirt registrada")?;
        if definicion.id_origen != destino.id_origen {
            bail!("el inventario de promoción no coincide con su origen declarado");
        }
        Ok(definicion)
    }

    fn virsh<I, S>(&self, argumentos: I) -> Result<SalidaComando>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut argumentos_finales = vec![OsString::from("--connect"), OsString::from(&self.uri)];
        argumentos_finales.extend(
            argumentos
                .into_iter()
                .map(|argumento| argumento.as_ref().to_os_string()),
        );
        self.ejecutor
            .ejecutar(Path::new(VIRSH), &argumentos_finales, PLAZO_COMANDO)
    }

    fn virsh_correcto<I, S>(&self, argumentos: I, contexto: &str) -> Result<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let salida = self.virsh(argumentos)?;
        if !salida.correcta {
            bail!("{contexto}: {}", sanear_error(&salida.error));
        }
        Ok(salida.salida)
    }

    fn ejecutar_correcto(
        &self,
        programa: &Path,
        argumentos: &[OsString],
        contexto: &str,
        plazo: Duration,
    ) -> Result<String> {
        let metadatos = fs::symlink_metadata(programa)
            .with_context(|| format!("no se encontró {}", programa.display()))?;
        if metadatos.file_type().is_symlink() || !metadatos.is_file() {
            bail!("el ejecutable auxiliar no es un fichero ordinario");
        }
        let salida = self.ejecutor.ejecutar(programa, argumentos, plazo)?;
        if !salida.correcta {
            bail!("{contexto}: {}", sanear_error(&salida.error));
        }
        Ok(salida.salida)
    }

    fn dumpxml(&self, dominio: &str) -> Result<Element> {
        let xml = self.virsh_correcto(
            ["dumpxml", "--inactive", dominio],
            "no se pudo leer la definición del dominio",
        )?;
        Element::parse(xml.as_bytes()).context("libvirt devolvió un XML de dominio no válido")
    }

    fn dumpxml_activo(&self, dominio: &str) -> Result<Element> {
        let xml = self.virsh_correcto(["dumpxml", dominio], "no se pudo leer el dominio activo")?;
        Element::parse(xml.as_bytes())
            .context("libvirt devolvió un XML de dominio activo no válido")
    }

    fn ruta_volumen(&self, pool: &str, volumen: &str) -> Result<String> {
        Ok(self
            .virsh_correcto(
                ["vol-path", "--pool", pool, volumen],
                "no se pudo resolver el volumen registrado",
            )?
            .trim()
            .to_owned())
    }

    fn capacidad_volumen(&self, pool: &str, volumen: &str) -> Result<u64> {
        let salida = self.virsh_correcto(
            ["vol-info", "--bytes", "--pool", pool, volumen],
            "no se pudo inspeccionar el volumen registrado",
        )?;
        campo_salida(&salida, "Capacity:")?
            .split_whitespace()
            .next()
            .context("capacidad vacía")?
            .parse()
            .context("capacidad de volumen no numérica")
    }

    fn pool_activo(&self, pool: &str) -> Result<bool> {
        let salida = self.virsh_correcto(
            ["pool-info", pool],
            "no se pudo inspeccionar el pool registrado",
        )?;
        Ok(campo_salida(&salida, "State:")? == "running")
    }

    fn nombre_volumen(id_instancia: &Identificador) -> String {
        format!("{}.qcow2", id_instancia.como_str())
    }

    fn volumen_existe(&self, pool: &str, nombre: &str) -> Result<bool> {
        let salida = self.virsh_correcto(
            ["vol-list", "--pool", pool],
            "no se pudieron enumerar los volúmenes del pool",
        )?;
        Ok(volumen_en_listado(&salida, nombre))
    }

    fn dominio_existe(&self, nombre: &str) -> Result<bool> {
        let salida = self.virsh_correcto(
            ["list", "--all", "--name"],
            "no se pudieron enumerar los dominios",
        )?;
        Ok(salida.lines().any(|linea| linea.trim() == nombre))
    }

    fn ultimo_fallo_arranque(&self, id_instancia: &Identificador) -> Option<CodigoFalloArranque> {
        let metadatos = fs::symlink_metadata(JOURNALCTL).ok()?;
        if metadatos.file_type().is_symlink() || !metadatos.is_file() {
            return None;
        }
        let argumentos = [
            "--no-pager",
            "--output=cat",
            "--lines=2000",
            "--unit=libvirtd",
            "--unit=virtqemud",
            "--unit=virtlogd",
        ]
        .map(OsString::from);
        let salida = self
            .ejecutor
            .ejecutar(Path::new(JOURNALCTL), &argumentos, PLAZO_COMANDO)
            .ok()?;
        if !salida.correcta {
            return None;
        }
        salida
            .salida
            .lines()
            .rev()
            .filter(|linea| linea.contains(id_instancia.como_str()))
            .find_map(clasificar_fallo_arranque)
    }

    fn crear_volumen_incremental(
        &self,
        definicion: &DefinicionPlantillaLibvirt,
        id_instancia: &Identificador,
    ) -> Result<String> {
        if !self.pool_activo(&definicion.pool_plantilla)?
            || !self.pool_activo(&definicion.pool_instancias)?
        {
            bail!("los pools registrados deben estar activos");
        }
        let origen =
            self.ruta_volumen(&definicion.pool_plantilla, &definicion.volumen_plantilla)?;
        let capacidad =
            self.capacidad_volumen(&definicion.pool_plantilla, &definicion.volumen_plantilla)?;
        let nombre = Self::nombre_volumen(id_instancia);
        self.virsh_correcto(
            [
                "vol-create-as",
                &definicion.pool_instancias,
                &nombre,
                &capacidad.to_string(),
                "--allocation",
                "0",
                "--format",
                "qcow2",
                "--backing-vol",
                &origen,
                "--backing-vol-format",
                "qcow2",
            ],
            "no se pudo crear el volumen incremental",
        )?;
        self.ruta_volumen(&definicion.pool_instancias, &nombre)
    }

    fn eliminar_volumen(
        &self,
        definicion: &DefinicionPlantillaLibvirt,
        id_instancia: &Identificador,
    ) -> Result<()> {
        let nombre = Self::nombre_volumen(id_instancia);
        if !self.volumen_existe(&definicion.pool_instancias, &nombre)? {
            return Ok(());
        }
        self.virsh_correcto(
            ["vol-delete", "--pool", &definicion.pool_instancias, &nombre],
            "no se pudo retirar el volumen exacto de la instancia",
        )?;
        Ok(())
    }

    fn definir_clon(
        &self,
        definicion: &DefinicionPlantillaLibvirt,
        id_instancia: &Identificador,
        volumen: &str,
    ) -> Result<HuellaIdentidad> {
        let mut dominio = self.dumpxml(&definicion.dominio)?;
        let huella_plantilla = huella_identidad(&dominio)?;
        sanear_clon(
            &mut dominio,
            id_instancia.como_str(),
            volumen,
            definicion.red.as_deref(),
        )?;
        self.definir_elemento(&dominio, id_instancia.como_str())?;
        Ok(huella_plantilla)
    }

    fn retirar_definicion(&self, id_instancia: &Identificador, dominio: &Element) -> Result<()> {
        self.retirar_definicion_nombre(id_instancia.como_str(), dominio)
    }

    fn retirar_definicion_nombre(&self, nombre: &str, dominio: &Element) -> Result<()> {
        let mut argumentos = vec![OsString::from("undefine"), OsString::from(nombre)];
        if dominio
            .get_child("os")
            .and_then(|valor| valor.get_child("nvram"))
            .is_some()
        {
            argumentos.push(OsString::from("--nvram"));
        }
        if dominio
            .get_child("devices")
            .and_then(|valor| valor.get_child("tpm"))
            .is_some()
        {
            argumentos.push(OsString::from("--tpm"));
        }
        let salida = self.virsh(argumentos)?;
        if !salida.correcta && self.dominio_existe(nombre)? {
            bail!(
                "no se pudo retirar la definición exacta: {}",
                sanear_error(&salida.error)
            );
        }
        Ok(())
    }

    fn red_aislada_disponible(&self, nombre: &str) -> Result<bool> {
        if !self.red_activa(nombre)? {
            return Ok(false);
        }
        let xml = self.virsh_correcto(
            ["net-dumpxml", nombre],
            "no se pudo inspeccionar la red registrada",
        )?;
        let red = Element::parse(xml.as_bytes()).context("XML de red no válido")?;
        Ok(red.get_child("forward").is_none())
    }

    fn red_activa(&self, nombre: &str) -> Result<bool> {
        let activas = self.virsh_correcto(
            ["net-list", "--name"],
            "no se pudieron enumerar las redes activas",
        )?;
        Ok(activas.lines().any(|linea| linea.trim() == nombre))
    }

    fn qga(
        &self,
        id_instancia: &Identificador,
        solicitud: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let solicitud = serde_json::to_string(&solicitud)?;
        let salida = self.virsh_correcto(
            ["qemu-agent-command", id_instancia.como_str(), &solicitud],
            "el canal autenticado del hipervisor rechazó la consulta",
        )?;
        let respuesta: serde_json::Value = serde_json::from_str(&salida)
            .context("el agente devolvió una respuesta JSON no válida")?;
        if respuesta.get("error").is_some() {
            bail!("el agente rechazó la consulta tipada");
        }
        Ok(respuesta)
    }

    fn leer_fichero_huesped_fijo(
        &self,
        id_instancia: &Identificador,
        ruta: &'static str,
        maximo_bytes: usize,
        etiqueta: &'static str,
    ) -> Result<Vec<u8>> {
        let apertura = self.qga(
            id_instancia,
            serde_json::json!({
                "execute": "guest-file-open",
                "arguments": {"path": ruta, "mode": "r"}
            }),
        )?;
        let manejador = apertura
            .get("return")
            .and_then(serde_json::Value::as_i64)
            .with_context(|| format!("el agente no devolvió un manejador de {etiqueta}"))?;
        let lectura = self.qga(
            id_instancia,
            serde_json::json!({
                "execute": "guest-file-read",
                "arguments": {"handle": manejador, "count": maximo_bytes + 1}
            }),
        );
        let cierre = self.qga(
            id_instancia,
            serde_json::json!({
                "execute": "guest-file-close",
                "arguments": {"handle": manejador}
            }),
        );
        let lectura = lectura?;
        cierre.with_context(|| format!("no se pudo cerrar {etiqueta}"))?;
        let retorno = lectura
            .get("return")
            .with_context(|| format!("el agente no devolvió el contenido de {etiqueta}"))?;
        if retorno.get("eof").and_then(serde_json::Value::as_bool) != Some(true) {
            bail!("{etiqueta} supera la cuota permitida");
        }
        let contenido = retorno
            .get("buf-b64")
            .and_then(serde_json::Value::as_str)
            .with_context(|| format!("{etiqueta} no contiene datos"))?;
        let contenido = STANDARD
            .decode(contenido)
            .with_context(|| format!("{etiqueta} no usa Base64 válido"))?;
        if contenido.len() > maximo_bytes {
            bail!("{etiqueta} supera la cuota permitida");
        }
        Ok(contenido)
    }

    fn direccion_observable_por(&self, id_instancia: &Identificador, fuente: &'static str) -> bool {
        self.virsh([
            "domifaddr",
            id_instancia.como_str(),
            "--source",
            fuente,
            "--full",
        ])
        .ok()
        .filter(|salida| salida.correcta)
        .and_then(|salida| extraer_direccion(&salida.salida))
        .is_some()
    }

    fn qga_disponible(&self, id_instancia: &Identificador) -> bool {
        self.qga(id_instancia, serde_json::json!({"execute": "guest-ping"}))
            .is_ok()
    }

    fn fichero_huesped_fijo_presente(
        &self,
        id_instancia: &Identificador,
        ruta: &'static str,
    ) -> bool {
        let apertura = self.qga(
            id_instancia,
            serde_json::json!({
                "execute": "guest-file-open",
                "arguments": {"path": ruta, "mode": "r"}
            }),
        );
        let Some(manejador) = apertura
            .ok()
            .and_then(|valor| valor.get("return").and_then(serde_json::Value::as_i64))
        else {
            return false;
        };
        self.qga(
            id_instancia,
            serde_json::json!({
                "execute": "guest-file-close",
                "arguments": {"handle": manejador}
            }),
        )
        .is_ok()
    }

    fn identidad_ssh_desde_qga(
        &self,
        recibo: &ReciboReserva,
        perfil: PerfilIdentidadAcceso,
    ) -> Result<IdentidadServidor> {
        let dominio = self.dumpxml_activo(recibo.id_instancia.como_str())?;
        let uuid_observado = validar_dominio_de_reserva(&dominio, recibo)?;
        let rutas = rutas_identidad_ssh(perfil);
        let marca = self.leer_fichero_huesped_fijo(
            &recibo.id_instancia,
            rutas.marca_uuid,
            MAXIMO_BYTES_MARCA_UUID,
            "la marca UUID de identidad SSH",
        )?;
        validar_marca_uuid(&marca, &uuid_observado)?;
        let clave = self.leer_fichero_huesped_fijo(
            &recibo.id_instancia,
            rutas.clave_publica,
            MAXIMO_BYTES_CLAVE_PUBLICA_SSH,
            "la clave pública SSH",
        )?;
        validar_clave_publica_openssh(&clave)
    }

    fn definicion_origen_recibo(
        &self,
        recibo: &ReciboReserva,
    ) -> Result<&DefinicionPlantillaLibvirt> {
        self.definiciones
            .get(&recibo.id_plantilla)
            .context("la reserva no conserva una plantilla de origen registrada")
    }

    fn ruta_incremental(&self, recibo: &ReciboReserva) -> Result<String> {
        let origen = self.definicion_origen_recibo(recibo)?;
        self.ruta_volumen(
            &origen.pool_instancias,
            &Self::nombre_volumen(&recibo.id_instancia),
        )
    }

    fn recursos_escritura_disponibles(
        &self,
        recibo: &ReciboReserva,
        dominio: &Element,
    ) -> Result<bool> {
        let definicion = self.definicion_origen_recibo(recibo)?;
        let nombre_volumen = Self::nombre_volumen(&recibo.id_instancia);
        let xml_volumen = self.virsh_correcto(
            [
                "vol-dumpxml",
                "--pool",
                &definicion.pool_instancias,
                &nombre_volumen,
            ],
            "no se pudo inspeccionar la cadena de almacenamiento",
        )?;
        let volumen = Element::parse(xml_volumen.as_bytes())
            .context("libvirt devolvió un XML de volumen no válido")?;
        let mut recursos_objetivo = recursos_escritura_dominio(dominio)?;
        recursos_objetivo.extend(rutas_backing_volumen(&volumen)?);

        let activas = self.virsh_correcto(
            ["list", "--name"],
            "no se pudieron enumerar las instancias activas",
        )?;
        for nombre in activas
            .lines()
            .map(str::trim)
            .filter(|valor| !valor.is_empty())
        {
            if nombre == recibo.id_instancia.como_str() {
                continue;
            }
            if nombre.len() > 128 || nombre.chars().any(char::is_control) {
                bail!("libvirt devolvió una identidad de dominio no válida");
            }
            let dominio_activo = self.dumpxml_activo(nombre)?;
            let recursos_activos = recursos_escritura_dominio(&dominio_activo)?;
            if recursos_objetivo
                .iter()
                .any(|recurso| recursos_activos.contains(recurso))
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn definir_elemento(&self, dominio: &Element, etiqueta: &str) -> Result<()> {
        let sufijo = CONTADOR_XML_TEMPORAL.fetch_add(1, Ordering::Relaxed);
        let temporal = std::env::temp_dir().join(format!(
            "laboratorio-libvirt-{}-{sufijo}-{etiqueta}.xml",
            std::process::id(),
        ));
        let mut opciones = OpenOptions::new();
        opciones.write(true).create_new(true);
        #[cfg(unix)]
        opciones.mode(0o600);
        let mut archivo = opciones
            .open(&temporal)
            .with_context(|| format!("existe o no puede crearse {}", temporal.display()))?;
        dominio.write(&mut archivo)?;
        archivo.write_all(b"\n")?;
        archivo.sync_all()?;
        let resultado = self.virsh_correcto(
            ["define", ruta_utf8(&temporal)?],
            "no se pudo registrar la definición exacta",
        );
        fs::remove_file(&temporal).with_context(|| {
            format!("no se pudo retirar el XML temporal {}", temporal.display())
        })?;
        resultado.map(|_| ())
    }

    fn eliminar_volumen_exacto(&self, pool: &str, volumen: &str) -> Result<()> {
        if !self.volumen_existe(pool, volumen)? {
            return Ok(());
        }
        self.virsh_correcto(
            ["vol-delete", "--pool", pool, volumen],
            "no se pudo retirar el volumen exacto de promoción",
        )?;
        Ok(())
    }

    fn validar_dominio_promovido(
        &self,
        definicion: &DefinicionPromocionLibvirt,
        ruta_volumen: &str,
    ) -> Result<()> {
        let dominio = self.dumpxml(&definicion.dominio_destino)?;
        if texto_hijo(&dominio, "name")? != definicion.dominio_destino
            || texto_hijo(&dominio, "uuid")?.to_ascii_lowercase() != definicion.uuid_destino
        {
            bail!("la identidad de la plantilla promovida no coincide con el inventario");
        }
        validar_clon_definido_sin_plantilla(
            &dominio,
            &definicion.dominio_destino,
            ruta_volumen,
            definicion.red_final.as_deref(),
        )?;
        let discos = discos_dominio(&dominio)?;
        if discos.len() != 1 || discos[0].destino != definicion.destino_disco {
            bail!("el disco promovido no coincide con el inventario");
        }
        if let Some(red) = definicion.red_final.as_deref() {
            if !self.red_aislada_disponible(red)? {
                bail!("la red final promovida no está aislada y disponible");
            }
        }
        Ok(())
    }

    fn validar_volumen_aplanado(&self, ruta: &str) -> Result<()> {
        let salida = self.ejecutar_correcto(
            Path::new(QEMU_IMG),
            &[
                OsString::from("info"),
                OsString::from("--output=json"),
                OsString::from(ruta),
            ],
            "no se pudo inspeccionar el volumen promovido",
            PLAZO_COMANDO,
        )?;
        let informacion: serde_json::Value =
            serde_json::from_str(&salida).context("qemu-img devolvió JSON no válido")?;
        if informacion
            .get("format")
            .and_then(serde_json::Value::as_str)
            != Some("qcow2")
            || informacion.get("backing-filename").is_some()
            || informacion.get("full-backing-filename").is_some()
        {
            bail!("el volumen promovido no es un qcow2 independiente");
        }
        Ok(())
    }
}

impl ProveedorInstancias for LibvirtCli {
    fn inspeccionar_plantilla(&self, plantilla: &Plantilla) -> Result<EstadoPlantilla> {
        let definicion = self.definicion(plantilla)?;
        let estado = self.virsh_correcto(
            ["domstate", &definicion.dominio],
            "no se pudo consultar la plantilla",
        )?;
        let informacion = self.virsh_correcto(
            ["dominfo", &definicion.dominio],
            "no se pudo consultar la identidad de la plantilla",
        )?;
        let dominio = self.dumpxml(&definicion.dominio)?;
        let discos = discos_dominio(&dominio)?;
        let disco_registrado = discos
            .iter()
            .find(|disco| disco.destino == definicion.destino_disco);
        let ruta_registrada =
            self.ruta_volumen(&definicion.pool_plantilla, &definicion.volumen_plantilla)?;
        let red_conforme = match (plantilla.politica_red, definicion.red.as_deref()) {
            (PoliticaRed::SinRed, None) => true,
            (PoliticaRed::Aislada, Some(red)) => self.red_aislada_disponible(red)?,
            _ => false,
        };
        Ok(EstadoPlantilla {
            apagada: estado.trim() == "shut off",
            sin_estado_guardado: campo_salida(&informacion, "Managed save:")? == "no",
            identidad_coincide: texto_hijo(&dominio, "uuid")?.to_ascii_lowercase()
                == definicion.uuid_esperado,
            discos_sistema: discos.len(),
            formato_incremental_compatible: disco_registrado
                .is_some_and(|disco| disco.formato == "qcow2"),
            origen_registrado: disco_registrado
                .is_some_and(|disco| disco.origen == ruta_registrada),
            red_conforme,
        })
    }

    fn preparar_instancia(&self, plantilla: &Plantilla, recibo: &ReciboReserva) -> Result<()> {
        let definicion = self.definicion(plantilla)?;
        if self.dominio_existe(recibo.id_instancia.como_str())? {
            bail!("ya existe la instancia exacta de la reserva");
        }
        let nombre_volumen = Self::nombre_volumen(&recibo.id_instancia);
        if self.volumen_existe(&definicion.pool_instancias, &nombre_volumen)? {
            bail!("ya existe el volumen exacto de la reserva");
        }
        let volumen = self.crear_volumen_incremental(definicion, &recibo.id_instancia)?;
        let preparacion = (|| -> Result<()> {
            let huella_plantilla = self.definir_clon(definicion, &recibo.id_instancia, &volumen)?;
            let clon = self.dumpxml(recibo.id_instancia.como_str())?;
            validar_clon_definido(
                &clon,
                recibo.id_instancia.como_str(),
                &volumen,
                definicion.red.as_deref(),
                &huella_plantilla,
            )
        })();
        if let Err(error_preparacion) = preparacion {
            if self.dominio_existe(recibo.id_instancia.como_str())? {
                let clon = self.dumpxml(recibo.id_instancia.como_str())?;
                self.retirar_definicion(&recibo.id_instancia, &clon)
                    .context("falló la preparación y tampoco pudo retirarse la definición")?;
            }
            self.eliminar_volumen(definicion, &recibo.id_instancia)
                .context("falló la preparación y tampoco pudo retirarse el volumen")?;
            return Err(error_preparacion);
        }
        Ok(())
    }

    fn estado_instancia(&self, id_instancia: &Identificador) -> Result<EstadoInstancia> {
        if !self.dominio_existe(id_instancia.como_str())? {
            return Ok(EstadoInstancia::Ausente);
        }
        let salida = self.virsh_correcto(
            ["domstate", id_instancia.como_str()],
            "no se pudo consultar el estado de la instancia",
        )?;
        Ok(if salida.trim() == "shut off" {
            EstadoInstancia::Apagada
        } else {
            EstadoInstancia::Encendida
        })
    }

    fn iniciar_instancia(&self, id_instancia: &Identificador) -> Result<()> {
        let salida = self.virsh(["start", id_instancia.como_str()])?;
        if !salida.correcta {
            let codigo = clasificar_fallo_arranque(&salida.error)
                .unwrap_or(CodigoFalloArranque::NoClasificado);
            bail!(
                "no se pudo iniciar la instancia: {}",
                etiqueta_fallo_arranque(codigo)
            );
        }
        Ok(())
    }

    fn solicitar_apagado(&self, id_instancia: &Identificador) -> Result<()> {
        self.virsh_correcto(
            ["shutdown", id_instancia.como_str()],
            "no se pudo solicitar el apagado cooperativo",
        )?;
        Ok(())
    }

    fn esperar_apagado(&self, id_instancia: &Identificador, plazo: Duration) -> Result<bool> {
        let inicio = Instant::now();
        loop {
            if self.estado_instancia(id_instancia)? == EstadoInstancia::Apagada {
                return Ok(true);
            }
            if inicio.elapsed() >= plazo {
                return Ok(false);
            }
            thread::sleep(Duration::from_millis(250));
        }
    }

    fn direccion_instancia(&self, id_instancia: &Identificador) -> Result<Option<IpAddr>> {
        if self.estado_instancia(id_instancia)? != EstadoInstancia::Encendida {
            return Ok(None);
        }
        for origen in ["lease", "agent", "arp"] {
            let salida = self.virsh([
                "domifaddr",
                id_instancia.como_str(),
                "--source",
                origen,
                "--full",
            ])?;
            if !salida.correcta {
                continue;
            }
            if let Some(direccion) = extraer_direccion(&salida.salida) {
                return Ok(Some(direccion));
            }
        }
        Ok(None)
    }

    fn identidad_servidor(
        &self,
        plantilla: &Plantilla,
        recibo: &ReciboReserva,
    ) -> Result<Option<IdentidadServidor>> {
        let definicion = self.definicion(plantilla)?;
        let Some(perfil) = definicion.perfil_identidad_acceso else {
            return Ok(None);
        };
        if plantilla.canal_acceso.as_ref().map(|canal| canal.protocolo)
            != Some(ProtocoloAcceso::Ssh)
        {
            bail!("el perfil de identidad solo puede emplearse con SSH");
        }
        Ok(Some(self.identidad_ssh_desde_qga(recibo, perfil)?))
    }

    fn retirar_instancia(&self, plantilla: &Plantilla, id_instancia: &Identificador) -> Result<()> {
        let definicion = self.definicion(plantilla)?;
        if self.estado_instancia(id_instancia)? == EstadoInstancia::Encendida {
            bail!("se rechaza retirar una instancia encendida");
        }
        if self.dominio_existe(id_instancia.como_str())? {
            let clon = self.dumpxml(id_instancia.como_str())?;
            let volumen = self.ruta_volumen(
                &definicion.pool_instancias,
                &Self::nombre_volumen(id_instancia),
            )?;
            validar_clon_definido_sin_plantilla(
                &clon,
                id_instancia.como_str(),
                &volumen,
                definicion.red.as_deref(),
            )?;
            self.retirar_definicion(id_instancia, &clon)?;
        }
        self.eliminar_volumen(definicion, id_instancia)
    }
}

impl ObservadorAcceso for LibvirtCli {
    fn observar_acceso(
        &self,
        plantilla: &Plantilla,
        recibo: &ReciboReserva,
    ) -> EstadoAccesoObservado {
        let instancia_encendida = self
            .estado_instancia(&recibo.id_instancia)
            .is_ok_and(|estado| estado == EstadoInstancia::Encendida);
        if !instancia_encendida {
            return EstadoAccesoObservado {
                instancia_encendida: false,
                direccion_lease_observable: false,
                direccion_agente_observable: false,
                direccion_arp_observable: false,
                qga_disponible: false,
                marca_uuid_presente: false,
                marca_uuid_coincidente: false,
                clave_servidor_ssh_presente: false,
                clave_servidor_ssh_valida: false,
            };
        }
        let direccion_lease_observable =
            self.direccion_observable_por(&recibo.id_instancia, "lease");
        let direccion_agente_observable =
            self.direccion_observable_por(&recibo.id_instancia, "agent");
        let direccion_arp_observable = self.direccion_observable_por(&recibo.id_instancia, "arp");
        let qga_disponible = self.qga_disponible(&recibo.id_instancia);
        let perfil = self
            .definicion(plantilla)
            .ok()
            .and_then(|definicion| definicion.perfil_identidad_acceso);
        let rutas = perfil.map(rutas_identidad_ssh);
        let marca_uuid_presente = qga_disponible
            && rutas.is_some_and(|rutas| {
                self.fichero_huesped_fijo_presente(&recibo.id_instancia, rutas.marca_uuid)
            });
        let clave_servidor_ssh_presente = qga_disponible
            && rutas.is_some_and(|rutas| {
                self.fichero_huesped_fijo_presente(&recibo.id_instancia, rutas.clave_publica)
            });
        let uuid_observado = self
            .dumpxml_activo(recibo.id_instancia.como_str())
            .and_then(|dominio| validar_dominio_de_reserva(&dominio, recibo))
            .ok();
        let marca_uuid_coincidente = marca_uuid_presente
            && rutas.is_some_and(|rutas| {
                uuid_observado.as_deref().is_some_and(|uuid| {
                    self.leer_fichero_huesped_fijo(
                        &recibo.id_instancia,
                        rutas.marca_uuid,
                        MAXIMO_BYTES_MARCA_UUID,
                        "la marca UUID de identidad SSH",
                    )
                    .and_then(|marca| validar_marca_uuid(&marca, uuid))
                    .is_ok()
                })
            });
        let clave_servidor_ssh_valida = clave_servidor_ssh_presente
            && rutas.is_some_and(|rutas| {
                self.leer_fichero_huesped_fijo(
                    &recibo.id_instancia,
                    rutas.clave_publica,
                    MAXIMO_BYTES_CLAVE_PUBLICA_SSH,
                    "la clave pública SSH",
                )
                .and_then(|clave| validar_clave_publica_openssh(&clave))
                .is_ok()
            });
        EstadoAccesoObservado {
            instancia_encendida,
            direccion_lease_observable,
            direccion_agente_observable,
            direccion_arp_observable,
            qga_disponible,
            marca_uuid_presente,
            marca_uuid_coincidente,
            clave_servidor_ssh_presente,
            clave_servidor_ssh_valida,
        }
    }
}

impl ObservadorArranque for LibvirtCli {
    fn observar_arranque(&self, recibo: &ReciboReserva) -> Result<EstadoArranqueObservado> {
        let instancia_registrada = self.dominio_existe(recibo.id_instancia.como_str())?;
        if !instancia_registrada {
            return Ok(EstadoArranqueObservado {
                instancia_registrada: false,
                instancia_apagada: false,
                definicion_valida: false,
                almacenamiento_presente: false,
                recursos_escritura_disponibles: false,
                redes_requeridas_activas: false,
                estado_guardado_ausente: false,
                ultimo_estado_fallido: false,
                ultimo_fallo: None,
            });
        }
        let estado = self.virsh_correcto(
            ["domstate", "--reason", recibo.id_instancia.como_str()],
            "no se pudo consultar la razón de estado",
        )?;
        let instancia_apagada = estado.lines().next().is_some_and(|linea| {
            linea.trim() == "shut off" || linea.trim().starts_with("shut off (")
        });
        let ultimo_estado_fallido = estado.to_ascii_lowercase().contains("failed");
        let dominio = self.dumpxml(recibo.id_instancia.como_str())?;
        let definicion_valida = texto_hijo(&dominio, "name")
            .is_ok_and(|nombre| nombre == recibo.id_instancia.como_str());
        let discos = discos_dominio(&dominio)?;
        let almacenamiento_presente = !discos.is_empty()
            && discos.iter().all(|disco| {
                fs::symlink_metadata(&disco.origen)
                    .is_ok_and(|metadatos| !metadatos.file_type().is_symlink())
            });
        let recursos_escritura_disponibles =
            self.recursos_escritura_disponibles(recibo, &dominio)?;
        let redes_requeridas_activas = redes_de_dominio(&dominio)?
            .iter()
            .all(|red| self.red_activa(red).unwrap_or(false));
        let informacion = self.virsh_correcto(
            ["dominfo", recibo.id_instancia.como_str()],
            "no se pudo consultar el estado guardado",
        )?;
        let estado_guardado_ausente = campo_salida(&informacion, "Managed save:")? == "no";
        let ultimo_fallo = ultimo_estado_fallido
            .then(|| self.ultimo_fallo_arranque(&recibo.id_instancia))
            .flatten();
        Ok(EstadoArranqueObservado {
            instancia_registrada,
            instancia_apagada,
            definicion_valida,
            almacenamiento_presente,
            recursos_escritura_disponibles,
            redes_requeridas_activas,
            estado_guardado_ausente,
            ultimo_estado_fallido,
            ultimo_fallo,
        })
    }
}

impl ProveedorPreparacionPlantillas for LibvirtCli {
    fn comprobar_destino_libre(&self, destino: &DestinoPromocion) -> Result<()> {
        let definicion = self.definicion_promocion(destino)?;
        if self.dominio_existe(&definicion.dominio_destino)?
            || self.volumen_existe(&definicion.pool_destino, &definicion.volumen_destino)?
        {
            bail!("el destino de promoción ya contiene un recurso; no se sobrescribirá");
        }
        Ok(())
    }

    fn validar_identidad_ssh_candidata(
        &self,
        recibo: &ReciboReserva,
        destino: &DestinoPromocion,
    ) -> Result<IdentidadServidor> {
        let definicion = self.definicion_promocion(destino)?;
        if destino
            .plantilla
            .canal_acceso
            .as_ref()
            .map(|canal| canal.protocolo)
            != Some(ProtocoloAcceso::Ssh)
        {
            if definicion.perfil_identidad_acceso.is_some() {
                bail!("un destino sin SSH no puede declarar identidad OpenSSH");
            }
            bail!("el destino no declara un canal SSH");
        }
        let perfil = definicion
            .perfil_identidad_acceso
            .context("el destino SSH no declara un perfil de identidad")?;
        self.identidad_ssh_desde_qga(recibo, perfil)
    }

    fn sanear_candidata(&self, recibo: &ReciboReserva, destino: &DestinoPromocion) -> Result<()> {
        if self.estado_instancia(&recibo.id_instancia)? != EstadoInstancia::Apagada {
            bail!("la candidata debe estar apagada para sanearla");
        }
        let definicion = self.definicion_promocion(destino)?;
        let volumen = self.ruta_incremental(recibo)?;
        let original = self.dumpxml(recibo.id_instancia.como_str())?;
        let medios = definicion
            .medios_temporales
            .iter()
            .map(|medio| {
                Ok((
                    medio.destino.clone(),
                    self.ruta_volumen(&medio.pool, &medio.volumen)?,
                ))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        let mut saneada = original.clone();
        sanear_candidata_xml(
            &mut saneada,
            recibo.id_instancia.como_str(),
            &volumen,
            definicion.red_preparacion.as_deref(),
            definicion.red_final.as_deref(),
            &medios,
        )?;
        self.definir_elemento(&saneada, recibo.id_instancia.como_str())?;
        let comprobacion = (|| -> Result<()> {
            let definida = self.dumpxml(recibo.id_instancia.como_str())?;
            validar_clon_definido_sin_plantilla(
                &definida,
                recibo.id_instancia.como_str(),
                &volumen,
                definicion.red_final.as_deref(),
            )
        })();
        if let Err(error) = comprobacion {
            self.definir_elemento(&original, "rollback-saneamiento")
                .context("falló el saneamiento y tampoco pudo restaurarse la definición")?;
            return Err(error).context("la candidata no superó la comprobación posterior");
        }
        Ok(())
    }

    fn inspeccionar_candidata(
        &self,
        recibo: &ReciboReserva,
        destino: &DestinoPromocion,
    ) -> Result<EstadoCandidataPlantilla> {
        let definicion = self.definicion_promocion(destino)?;
        let estado = self.estado_instancia(&recibo.id_instancia)?;
        if estado == EstadoInstancia::Ausente {
            bail!("la candidata registrada no existe");
        }
        let dominio = self.dumpxml(recibo.id_instancia.como_str())?;
        let volumen = self.ruta_incremental(recibo)?;
        let discos = discos_dominio(&dominio)?;
        let dispositivos = dominio
            .get_child("devices")
            .context("candidata sin dispositivos")?;
        let sin_medios_temporales = !dispositivos.children.iter().any(|nodo| {
            matches!(nodo, XMLNode::Element(elemento)
                if elemento.name == "disk"
                    && elemento.attributes.get("device").map(String::as_str) != Some("disk"))
        });
        let red_final_conforme = red_dominio_conforme(&dominio, definicion.red_final.as_deref())
            && match definicion.red_final.as_deref() {
                Some(red) => self.red_aislada_disponible(red)?,
                None => true,
            };
        let origen = self.definicion_origen_recibo(recibo)?;
        let huella_origen = huella_identidad(&self.dumpxml(&origen.dominio)?)?;
        let huella_candidata = huella_identidad(&dominio)?;
        let identidad_independiente = huella_candidata.uuid != huella_origen.uuid
            && !identidad_reutilizada(&huella_candidata.nvram, &huella_origen.nvram)
            && !identidad_reutilizada(&huella_candidata.tpm, &huella_origen.tpm)
            && !huella_candidata
                .macs
                .iter()
                .any(|mac| huella_origen.macs.contains(mac));
        Ok(EstadoCandidataPlantilla {
            apagada: estado == EstadoInstancia::Apagada,
            sin_medios_temporales,
            red_final_conforme,
            disco_sistema_unico: discos.len() == 1 && discos[0].origen == volumen,
            identidad_independiente,
        })
    }

    fn promover_candidata(&self, recibo: &ReciboReserva, destino: &DestinoPromocion) -> Result<()> {
        if recibo
            .preparacion_plantilla
            .as_ref()
            .map(|progreso| progreso.fase)
            != Some(FasePreparacionPlantilla::PromocionEnCurso)
        {
            bail!("falta el recibo persistido de promoción en curso");
        }
        let definicion = self.definicion_promocion(destino)?;
        let origen = self.definicion_origen_recibo(recibo)?;
        let nombre_origen = Self::nombre_volumen(&recibo.id_instancia);
        let destino_dominio_existe = self.dominio_existe(&definicion.dominio_destino)?;
        let destino_volumen_existe =
            self.volumen_existe(&definicion.pool_destino, &definicion.volumen_destino)?;

        if destino_dominio_existe {
            if !destino_volumen_existe {
                bail!("existe el dominio destino sin su volumen inventariado");
            }
            let ruta_destino =
                self.ruta_volumen(&definicion.pool_destino, &definicion.volumen_destino)?;
            self.validar_volumen_aplanado(&ruta_destino)?;
            self.validar_dominio_promovido(definicion, &ruta_destino)?;
            if self.dominio_existe(recibo.id_instancia.como_str())? {
                if self.estado_instancia(&recibo.id_instancia)? == EstadoInstancia::Encendida {
                    bail!("la candidata volvió a encenderse; no se retiró su definición");
                }
                let clon = self.dumpxml(recibo.id_instancia.como_str())?;
                self.retirar_definicion(&recibo.id_instancia, &clon)?;
            }
            self.eliminar_volumen_exacto(&origen.pool_instancias, &nombre_origen)?;
            return Ok(());
        }
        if self.estado_instancia(&recibo.id_instancia)? != EstadoInstancia::Apagada {
            bail!("la candidata debe existir y estar apagada para promoverla");
        }
        let estado_candidata = self.inspeccionar_candidata(recibo, destino)?;
        if !estado_candidata.apagada
            || !estado_candidata.sin_medios_temporales
            || !estado_candidata.red_final_conforme
            || !estado_candidata.disco_sistema_unico
            || !estado_candidata.identidad_independiente
        {
            bail!("la candidata dejó de cumplir las invariantes antes de promoverla");
        }
        if destino_volumen_existe {
            self.eliminar_volumen_exacto(&definicion.pool_destino, &definicion.volumen_destino)?;
        }
        if !self.pool_activo(&definicion.pool_destino)? {
            bail!("el pool de promoción registrado no está activo");
        }
        let capacidad = self.capacidad_volumen(&origen.pool_instancias, &nombre_origen)?;
        self.virsh_correcto(
            [
                "vol-create-as",
                &definicion.pool_destino,
                &definicion.volumen_destino,
                &capacidad.to_string(),
                "--allocation",
                "0",
                "--format",
                "qcow2",
            ],
            "no se pudo crear el volumen de la nueva plantilla",
        )?;
        let ruta_origen = self.ruta_volumen(&origen.pool_instancias, &nombre_origen)?;
        let ruta_destino =
            self.ruta_volumen(&definicion.pool_destino, &definicion.volumen_destino)?;
        let conversion = self.ejecutar_correcto(
            Path::new(QEMU_IMG),
            &[
                OsString::from("convert"),
                OsString::from("-n"),
                OsString::from("-O"),
                OsString::from("qcow2"),
                OsString::from(&ruta_origen),
                OsString::from(&ruta_destino),
            ],
            "no se pudo aplanar el disco de la nueva plantilla",
            PLAZO_PROMOCION,
        );
        if let Err(error) = conversion {
            self.eliminar_volumen_exacto(&definicion.pool_destino, &definicion.volumen_destino)
                .context("falló la conversión y tampoco pudo retirarse el volumen incompleto")?;
            return Err(error);
        }
        if let Err(error) = self.validar_volumen_aplanado(&ruta_destino) {
            self.eliminar_volumen_exacto(&definicion.pool_destino, &definicion.volumen_destino)
                .context("el volumen no quedó independiente y tampoco pudo retirarse")?;
            return Err(error);
        }

        let original = self.dumpxml(recibo.id_instancia.como_str())?;
        let mut promovida = original.clone();
        preparar_xml_promocion(&mut promovida, definicion, &ruta_destino)?;
        if self.estado_instancia(&recibo.id_instancia)? != EstadoInstancia::Apagada {
            self.eliminar_volumen_exacto(&definicion.pool_destino, &definicion.volumen_destino)?;
            bail!("la candidata volvió a encenderse durante la promoción");
        }
        self.retirar_definicion(&recibo.id_instancia, &original)?;
        let definicion_destino = self.definir_elemento(&promovida, &definicion.dominio_destino);
        let validacion = definicion_destino
            .and_then(|_| self.validar_dominio_promovido(definicion, &ruta_destino));
        if let Err(error) = validacion {
            if self.dominio_existe(&definicion.dominio_destino)? {
                let dominio = self.dumpxml(&definicion.dominio_destino)?;
                self.retirar_definicion_nombre(&definicion.dominio_destino, &dominio)?;
            }
            self.definir_elemento(&original, "rollback-promocion")
                .context("falló la promoción y tampoco pudo restaurarse la candidata")?;
            self.eliminar_volumen_exacto(&definicion.pool_destino, &definicion.volumen_destino)
                .context("falló la promoción y tampoco pudo retirarse el volumen destino")?;
            return Err(error).context("la nueva plantilla no superó su validación");
        }
        self.eliminar_volumen_exacto(&origen.pool_instancias, &nombre_origen)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
struct RutasIdentidadSsh {
    marca_uuid: &'static str,
    clave_publica: &'static str,
}

fn rutas_identidad_ssh(perfil: PerfilIdentidadAcceso) -> RutasIdentidadSsh {
    match perfil {
        PerfilIdentidadAcceso::WindowsOpenssh => RutasIdentidadSsh {
            marca_uuid: r"C:\ProgramData\LaboratorioAplicaciones\estado\uuid-identidad-ssh.txt",
            clave_publica: r"C:\ProgramData\ssh\ssh_host_ed25519_key.pub",
        },
        PerfilIdentidadAcceso::LinuxOpenssh => RutasIdentidadSsh {
            marca_uuid: "/var/lib/laboratorio-libvirt/identidad-ssh.uuid",
            clave_publica: "/etc/ssh/ssh_host_ed25519_key.pub",
        },
    }
}

fn validar_dominio_de_reserva(dominio: &Element, recibo: &ReciboReserva) -> Result<String> {
    let nombre = texto_hijo(dominio, "name")?;
    if nombre != recibo.id_instancia.como_str() {
        bail!("el dominio consultado no coincide con la instancia reservada");
    }
    let uuid = texto_hijo(dominio, "uuid")?;
    validar_uuid_canonico(&uuid).context("el dominio no publica un UUID canónico")?;
    Ok(uuid)
}

fn validar_marca_uuid(contenido: &[u8], uuid_observado: &str) -> Result<()> {
    validar_uuid_canonico(uuid_observado).context("el dominio no publica un UUID canónico")?;
    let marca = linea_canonica(contenido, "la marca UUID")?;
    validar_uuid_canonico(marca).context("la marca de identidad no contiene un UUID canónico")?;
    if marca != uuid_observado {
        bail!("la marca UUID no coincide con el dominio reservado");
    }
    Ok(())
}

fn validar_uuid_canonico(valor: &str) -> Result<()> {
    if valor.len() != 36
        || !valor.bytes().enumerate().all(|(indice, byte)| {
            if matches!(indice, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')
            }
        })
    {
        bail!("UUID no canónico");
    }
    Ok(())
}

fn validar_clave_publica_openssh(contenido: &[u8]) -> Result<IdentidadServidor> {
    let clave = linea_canonica(contenido, "la clave pública SSH")?;
    let mut partes = clave.split(' ');
    let algoritmo_texto = partes.next().context("clave pública SSH sin algoritmo")?;
    let cuerpo = partes.next().context("clave pública SSH sin blob")?;
    let comentario = partes.next();
    if partes.next().is_some()
        || algoritmo_texto != "ssh-ed25519"
        || cuerpo.is_empty()
        || comentario.is_some_and(|valor| {
            valor.is_empty()
                || valor.len() > 128
                || !valor.bytes().all(|byte| byte.is_ascii_graphic())
        })
    {
        bail!("la clave pública SSH no es una clave Ed25519 canónica");
    }
    let binario = STANDARD
        .decode(cuerpo)
        .context("la clave pública SSH no usa Base64 válido")?;
    if STANDARD.encode(&binario) != cuerpo {
        bail!("el blob de la clave pública SSH no usa Base64 canónico");
    }
    let mut resto = binario.as_slice();
    let algoritmo_blob = leer_cadena_ssh(&mut resto)?;
    let clave_blob = leer_cadena_ssh(&mut resto)?;
    if algoritmo_blob != b"ssh-ed25519" || clave_blob.len() != 32 || !resto.is_empty() {
        bail!("el blob de la clave pública SSH no tiene estructura Ed25519 válida");
    }
    let huella = STANDARD_NO_PAD.encode(Sha256::digest(&binario));
    Ok(IdentidadServidor {
        algoritmo: Identificador::nuevo(algoritmo_texto)?,
        clave_publica: format!("{algoritmo_texto} {cuerpo}"),
        huella_sha256: format!("SHA256:{huella}"),
    })
}

fn linea_canonica<'a>(contenido: &'a [u8], etiqueta: &str) -> Result<&'a str> {
    let contenido = contenido
        .strip_suffix(b"\r\n")
        .or_else(|| contenido.strip_suffix(b"\n"))
        .unwrap_or(contenido);
    if contenido.is_empty() || contenido.contains(&b'\r') || contenido.contains(&b'\n') {
        bail!("{etiqueta} no contiene una única línea");
    }
    let texto =
        std::str::from_utf8(contenido).with_context(|| format!("{etiqueta} no es UTF-8"))?;
    if texto.trim() != texto {
        bail!("{etiqueta} contiene espacios no canónicos");
    }
    Ok(texto)
}

fn leer_cadena_ssh<'a>(resto: &mut &'a [u8]) -> Result<&'a [u8]> {
    let cabecera: [u8; 4] = resto
        .get(..4)
        .context("el blob SSH está truncado")?
        .try_into()
        .map_err(|_| anyhow::anyhow!("el blob SSH está truncado"))?;
    let longitud = u32::from_be_bytes(cabecera) as usize;
    let fin = 4_usize
        .checked_add(longitud)
        .context("la longitud del blob SSH desborda")?;
    let valor = resto
        .get(4..fin)
        .context("el blob SSH declara una longitud imposible")?;
    *resto = resto
        .get(fin..)
        .context("el blob SSH declara una longitud imposible")?;
    Ok(valor)
}

#[derive(Debug, Clone)]
struct DiscoDominio {
    destino: String,
    formato: String,
    origen: String,
}

#[derive(Debug, Clone, Default)]
struct HuellaIdentidad {
    uuid: String,
    nvram: Option<String>,
    tpm: Option<String>,
    macs: Vec<String>,
}

fn discos_dominio(dominio: &Element) -> Result<Vec<DiscoDominio>> {
    let dispositivos = dominio
        .get_child("devices")
        .context("dominio sin dispositivos")?;
    dispositivos
        .children
        .iter()
        .filter_map(|nodo| match nodo {
            XMLNode::Element(elemento)
                if elemento.name == "disk"
                    && elemento.attributes.get("device").map(String::as_str) == Some("disk") =>
            {
                Some(elemento)
            }
            _ => None,
        })
        .map(|disco| {
            let destino = disco
                .get_child("target")
                .and_then(|elemento| elemento.attributes.get("dev"))
                .cloned()
                .context("disco sin destino")?;
            let formato = disco
                .get_child("driver")
                .and_then(|elemento| elemento.attributes.get("type"))
                .cloned()
                .context("disco sin formato")?;
            let origen = disco
                .get_child("source")
                .and_then(|elemento| elemento.attributes.get("file"))
                .cloned()
                .context("disco sin fichero de origen")?;
            Ok(DiscoDominio {
                destino,
                formato,
                origen,
            })
        })
        .collect()
}

fn redes_de_dominio(dominio: &Element) -> Result<Vec<String>> {
    let dispositivos = dominio
        .get_child("devices")
        .context("dominio sin dispositivos")?;
    dispositivos
        .children
        .iter()
        .filter_map(|nodo| match nodo {
            XMLNode::Element(elemento) if elemento.name == "interface" => Some(elemento),
            _ => None,
        })
        .map(|interfaz| {
            if interfaz.attributes.get("type").map(String::as_str) != Some("network") {
                bail!("la instancia contiene una interfaz fuera del inventario de redes");
            }
            interfaz
                .get_child("source")
                .and_then(|origen| origen.attributes.get("network"))
                .cloned()
                .context("interfaz sin red de origen")
        })
        .collect()
}

fn recursos_escritura_dominio(dominio: &Element) -> Result<BTreeSet<String>> {
    let dispositivos = dominio
        .get_child("devices")
        .context("dominio sin dispositivos")?;
    let mut recursos = BTreeSet::new();
    for disco in dispositivos.children.iter().filter_map(|nodo| match nodo {
        XMLNode::Element(elemento) if elemento.name == "disk" => Some(elemento),
        _ => None,
    }) {
        if disco.get_child("readonly").is_some() {
            continue;
        }
        let origen = disco
            .get_child("source")
            .and_then(|elemento| {
                elemento
                    .attributes
                    .get("file")
                    .or_else(|| elemento.attributes.get("dev"))
            })
            .context("dispositivo de escritura sin origen")?;
        recursos.insert(origen.clone());
    }
    if let Some(nvram) = dominio
        .get_child("os")
        .and_then(|os| os.get_child("nvram"))
        .and_then(Element::get_text)
        .map(|texto| texto.trim().to_owned())
        .filter(|texto| !texto.is_empty())
    {
        recursos.insert(nvram);
    }
    Ok(recursos)
}

fn rutas_backing_volumen(volumen: &Element) -> Result<BTreeSet<String>> {
    let mut rutas = BTreeSet::new();
    let mut actual = volumen.get_child("backingStore");
    while let Some(backing) = actual {
        if let Some(ruta) = backing
            .get_child("path")
            .and_then(Element::get_text)
            .map(|texto| texto.trim().to_owned())
            .filter(|texto| !texto.is_empty())
        {
            rutas.insert(ruta);
        }
        actual = backing.get_child("backingStore");
    }
    if rutas.is_empty() {
        bail!("el volumen incremental no declara su cadena de respaldo");
    }
    Ok(rutas)
}

fn huella_identidad(dominio: &Element) -> Result<HuellaIdentidad> {
    let dispositivos = dominio
        .get_child("devices")
        .context("dominio sin dispositivos")?;
    let nvram = dominio
        .get_child("os")
        .and_then(|valor| valor.get_child("nvram"))
        .and_then(Element::get_text)
        .map(|valor| valor.trim().to_owned())
        .filter(|valor| !valor.is_empty());
    let tpm = dispositivos
        .get_child("tpm")
        .and_then(|valor| valor.get_child("backend"))
        .and_then(|valor| valor.get_child("source"))
        .and_then(|valor| valor.attributes.get("path"))
        .cloned();
    let macs = dispositivos
        .children
        .iter()
        .filter_map(|nodo| match nodo {
            XMLNode::Element(elemento) if elemento.name == "interface" => elemento
                .get_child("mac")
                .and_then(|valor| valor.attributes.get("address"))
                .cloned(),
            _ => None,
        })
        .collect();
    Ok(HuellaIdentidad {
        uuid: texto_hijo(dominio, "uuid")?,
        nvram,
        tpm,
        macs,
    })
}

fn sanear_clon(
    dominio: &mut Element,
    nombre: &str,
    volumen: &str,
    red: Option<&str>,
) -> Result<()> {
    dominio.children.retain(|nodo| {
        !matches!(nodo, XMLNode::Element(elemento)
            if matches!(elemento.name.as_str(), "uuid" | "seclabel")
                || elemento.name.ends_with("commandline"))
    });
    establecer_texto_hijo(dominio, "name", nombre)?;
    if let Some(os) = dominio.get_mut_child("os") {
        if let Some(nvram) = os.get_mut_child("nvram") {
            nvram.children.clear();
        }
    }
    let dispositivos = dominio
        .get_mut_child("devices")
        .context("dominio sin dispositivos")?;
    dispositivos.children.retain(|nodo| match nodo {
        XMLNode::Element(elemento) => match elemento.name.as_str() {
            "filesystem" | "hostdev" | "redirdev" => false,
            "channel" => elemento.attributes.get("type").map(String::as_str) != Some("spicevmc"),
            "disk" => elemento.attributes.get("device").map(String::as_str) == Some("disk"),
            "interface" => red.is_some(),
            _ => true,
        },
        _ => true,
    });
    let mut discos = 0_usize;
    let mut interfaces = 0_usize;
    for nodo in &mut dispositivos.children {
        let XMLNode::Element(elemento) = nodo else {
            continue;
        };
        match elemento.name.as_str() {
            "disk" => {
                discos += 1;
                let origen = elemento
                    .get_mut_child("source")
                    .context("disco sin origen")?;
                origen.attributes.clear();
                origen
                    .attributes
                    .insert("file".to_owned(), volumen.to_owned());
            }
            "interface" => {
                interfaces += 1;
                elemento
                    .children
                    .retain(|hijo| !matches!(hijo, XMLNode::Element(valor) if valor.name == "mac"));
                elemento
                    .attributes
                    .insert("type".to_owned(), "network".to_owned());
                let origen = elemento
                    .get_mut_child("source")
                    .context("interfaz sin origen")?;
                origen.attributes.clear();
                origen.attributes.insert(
                    "network".to_owned(),
                    red.context("falta la red registrada")?.to_owned(),
                );
            }
            "channel" => {
                elemento.children.retain(
                    |hijo| !matches!(hijo, XMLNode::Element(valor) if valor.name == "source"),
                );
            }
            "tpm" => {
                if let Some(backend) = elemento.get_mut_child("backend") {
                    backend.children.retain(
                        |hijo| !matches!(hijo, XMLNode::Element(valor) if valor.name == "source"),
                    );
                }
            }
            "graphics" if elemento.attributes.get("type").map(String::as_str) == Some("spice") => {
                elemento.children.retain(|hijo| {
                    !matches!(hijo, XMLNode::Element(valor)
                        if matches!(valor.name.as_str(), "clipboard" | "filetransfer"))
                });
                elemento.children.push(XMLNode::Element(elemento_simple(
                    "clipboard",
                    "copypaste",
                    "no",
                )));
                elemento.children.push(XMLNode::Element(elemento_simple(
                    "filetransfer",
                    "enable",
                    "no",
                )));
            }
            _ => {}
        }
    }
    if discos != 1 || (red.is_some() && interfaces != 1) || (red.is_none() && interfaces != 0) {
        bail!("la plantilla no contiene la topología admitida de disco y red");
    }
    Ok(())
}

fn validar_clon_definido(
    dominio: &Element,
    nombre: &str,
    volumen: &str,
    red: Option<&str>,
    huella_plantilla: &HuellaIdentidad,
) -> Result<()> {
    validar_clon_definido_sin_plantilla(dominio, nombre, volumen, red)?;
    let huella_clon = huella_identidad(dominio)?;
    if huella_clon.uuid == huella_plantilla.uuid
        || identidad_reutilizada(&huella_clon.nvram, &huella_plantilla.nvram)
        || identidad_reutilizada(&huella_clon.tpm, &huella_plantilla.tpm)
        || huella_clon
            .macs
            .iter()
            .any(|mac| huella_plantilla.macs.contains(mac))
    {
        bail!("la instancia reutiliza una identidad o estado de la plantilla");
    }
    Ok(())
}

fn identidad_reutilizada(actual: &Option<String>, plantilla: &Option<String>) -> bool {
    actual.is_some() && actual == plantilla
}

fn validar_clon_definido_sin_plantilla(
    dominio: &Element,
    nombre: &str,
    volumen: &str,
    red: Option<&str>,
) -> Result<()> {
    if texto_hijo(dominio, "name")? != nombre {
        bail!("la instancia definida no conserva su nombre exacto");
    }
    if dominio.children.iter().any(
        |nodo| matches!(nodo, XMLNode::Element(elemento) if elemento.name.ends_with("commandline")),
    ) {
        bail!("la instancia contiene una extensión de línea de órdenes no permitida");
    }
    let discos = discos_dominio(dominio)?;
    if discos.len() != 1 || discos[0].origen != volumen {
        bail!("la instancia no usa exclusivamente su volumen incremental");
    }
    let dispositivos = dominio
        .get_child("devices")
        .context("instancia sin dispositivos")?;
    if dispositivos.children.iter().any(|nodo| {
        matches!(nodo, XMLNode::Element(elemento)
            if elemento.name == "disk"
                && elemento.attributes.get("device").map(String::as_str) != Some("disk"))
    }) {
        bail!("la instancia conserva un dispositivo de almacenamiento extraíble");
    }
    if dispositivos.children.iter().any(|nodo| {
        matches!(nodo, XMLNode::Element(elemento)
            if matches!(elemento.name.as_str(), "filesystem" | "hostdev" | "redirdev")
                || (elemento.name == "channel"
                    && elemento.attributes.get("type").map(String::as_str) == Some("spicevmc")))
    }) {
        bail!("la instancia conserva un dispositivo compartido no permitido");
    }
    let interfaces = dispositivos
        .children
        .iter()
        .filter_map(|nodo| match nodo {
            XMLNode::Element(elemento) if elemento.name == "interface" => Some(elemento),
            _ => None,
        })
        .collect::<Vec<_>>();
    match red {
        None if interfaces.is_empty() => {}
        Some(nombre_red)
            if interfaces.len() == 1
                && interfaces[0]
                    .get_child("source")
                    .and_then(|origen| origen.attributes.get("network"))
                    .map(String::as_str)
                    == Some(nombre_red)
                && interfaces[0]
                    .get_child("mac")
                    .and_then(|mac| mac.attributes.get("address"))
                    .is_some() => {}
        _ => bail!("la red de la instancia no coincide con la política registrada"),
    }
    for graphics in dispositivos.children.iter().filter_map(|nodo| match nodo {
        XMLNode::Element(elemento)
            if elemento.name == "graphics"
                && elemento.attributes.get("type").map(String::as_str) == Some("spice") =>
        {
            Some(elemento)
        }
        _ => None,
    }) {
        if graphics
            .get_child("clipboard")
            .and_then(|valor| valor.attributes.get("copypaste"))
            .map(String::as_str)
            != Some("no")
            || graphics
                .get_child("filetransfer")
                .and_then(|valor| valor.attributes.get("enable"))
                .map(String::as_str)
                != Some("no")
        {
            bail!("SPICE conserva portapapeles o transferencia de ficheros");
        }
    }
    Ok(())
}

fn sanear_candidata_xml(
    dominio: &mut Element,
    nombre: &str,
    volumen_sistema: &str,
    red_preparacion: Option<&str>,
    red_final: Option<&str>,
    medios_permitidos: &BTreeMap<String, String>,
) -> Result<()> {
    if texto_hijo(dominio, "name")? != nombre {
        bail!("la candidata no conserva la identidad de la reserva");
    }
    let discos = discos_dominio(dominio)?;
    if discos.len() != 1 || discos[0].origen != volumen_sistema {
        bail!("la candidata no conserva exclusivamente su disco inventariado");
    }
    let dispositivos = dominio
        .get_mut_child("devices")
        .context("candidata sin dispositivos")?;
    for nodo in &dispositivos.children {
        let XMLNode::Element(elemento) = nodo else {
            continue;
        };
        if elemento.name != "disk"
            || elemento.attributes.get("device").map(String::as_str) == Some("disk")
        {
            continue;
        }
        let destino = elemento
            .get_child("target")
            .and_then(|valor| valor.attributes.get("dev"))
            .context("medio temporal sin destino")?;
        let origen = elemento
            .get_child("source")
            .and_then(|valor| {
                valor
                    .attributes
                    .get("file")
                    .or_else(|| valor.attributes.get("dev"))
            })
            .context("medio temporal sin origen")?;
        if medios_permitidos.get(destino) != Some(origen) {
            bail!("la candidata contiene un medio no inventariado");
        }
        if elemento.get_child("readonly").is_none() {
            bail!("la candidata contiene un medio temporal con escritura habilitada");
        }
    }
    dispositivos.children.retain(|nodo| {
        !matches!(nodo, XMLNode::Element(elemento)
            if elemento.name == "disk"
                && elemento.attributes.get("device").map(String::as_str) != Some("disk"))
    });

    let interfaces = dispositivos
        .children
        .iter_mut()
        .filter_map(|nodo| match nodo {
            XMLNode::Element(elemento) if elemento.name == "interface" => Some(elemento),
            _ => None,
        })
        .collect::<Vec<_>>();
    if interfaces.len() > 1 {
        bail!("la candidata contiene más de una interfaz de red");
    }
    match (interfaces.into_iter().next(), red_preparacion, red_final) {
        (None, _, None) => {}
        (Some(interfaz), temporal, Some(final_)) => {
            let origen = interfaz
                .get_mut_child("source")
                .context("interfaz sin origen")?;
            let actual = origen
                .attributes
                .get("network")
                .map(String::as_str)
                .context("interfaz sin red")?;
            if Some(actual) != temporal && actual != final_ {
                bail!("la candidata está conectada a una red no inventariada");
            }
            origen.attributes.clear();
            origen
                .attributes
                .insert("network".to_owned(), final_.to_owned());
        }
        (Some(interfaz), Some(temporal), None) => {
            let actual = interfaz
                .get_child("source")
                .and_then(|origen| origen.attributes.get("network"))
                .map(String::as_str);
            if actual != Some(temporal) {
                bail!("la candidata está conectada a una red no inventariada");
            }
            dispositivos.children.retain(
                |nodo| !matches!(nodo, XMLNode::Element(elemento) if elemento.name == "interface"),
            );
        }
        _ => bail!("la topología de red no coincide con el inventario de preparación"),
    }
    Ok(())
}

fn red_dominio_conforme(dominio: &Element, red: Option<&str>) -> bool {
    let Some(dispositivos) = dominio.get_child("devices") else {
        return false;
    };
    let interfaces = dispositivos
        .children
        .iter()
        .filter_map(|nodo| match nodo {
            XMLNode::Element(elemento) if elemento.name == "interface" => Some(elemento),
            _ => None,
        })
        .collect::<Vec<_>>();
    match red {
        None => interfaces.is_empty(),
        Some(nombre) => {
            interfaces.len() == 1
                && interfaces[0]
                    .get_child("source")
                    .and_then(|origen| origen.attributes.get("network"))
                    .map(String::as_str)
                    == Some(nombre)
        }
    }
}

fn preparar_xml_promocion(
    dominio: &mut Element,
    definicion: &DefinicionPromocionLibvirt,
    volumen: &str,
) -> Result<()> {
    establecer_texto_hijo(dominio, "name", &definicion.dominio_destino)?;
    establecer_texto_hijo(dominio, "uuid", &definicion.uuid_destino)?;
    let dispositivos = dominio
        .get_mut_child("devices")
        .context("candidata sin dispositivos")?;
    let discos = dispositivos
        .children
        .iter_mut()
        .filter_map(|nodo| match nodo {
            XMLNode::Element(elemento)
                if elemento.name == "disk"
                    && elemento.attributes.get("device").map(String::as_str) == Some("disk") =>
            {
                Some(elemento)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    if discos.len() != 1 {
        bail!("la candidata no contiene un único disco de sistema");
    }
    let disco = discos.into_iter().next().expect("se comprobó un disco");
    let origen = disco.get_mut_child("source").context("disco sin origen")?;
    origen.attributes.clear();
    origen
        .attributes
        .insert("file".to_owned(), volumen.to_owned());
    let destino = disco.get_mut_child("target").context("disco sin destino")?;
    destino
        .attributes
        .insert("dev".to_owned(), definicion.destino_disco.clone());
    Ok(())
}

fn texto_hijo(elemento: &Element, nombre: &str) -> Result<String> {
    elemento
        .get_child(nombre)
        .and_then(Element::get_text)
        .map(|texto| texto.trim().to_owned())
        .context("elemento XML sin texto esperado")
}

fn establecer_texto_hijo(elemento: &mut Element, nombre: &str, valor: &str) -> Result<()> {
    let hijo = elemento
        .get_mut_child(nombre)
        .with_context(|| format!("elemento XML sin {nombre}"))?;
    hijo.children.clear();
    hijo.children.push(XMLNode::Text(valor.to_owned()));
    Ok(())
}

fn elemento_simple(nombre: &str, atributo: &str, valor: &str) -> Element {
    let mut elemento = Element::new(nombre);
    elemento
        .attributes
        .insert(atributo.to_owned(), valor.to_owned());
    elemento
}

fn campo_salida<'a>(salida: &'a str, campo: &str) -> Result<&'a str> {
    salida
        .lines()
        .find_map(|linea| linea.trim().strip_prefix(campo).map(str::trim))
        .with_context(|| format!("la salida no contiene {campo}"))
}

fn ruta_utf8(ruta: &Path) -> Result<&str> {
    ruta.to_str().context("la ruta temporal no es UTF-8")
}

fn sanear_error(error: &str) -> String {
    let primera = error.lines().next().unwrap_or("error no detallado").trim();
    primera.chars().take(240).collect()
}

fn clasificar_fallo_arranque(error: &str) -> Option<CodigoFalloArranque> {
    let error = error.to_ascii_lowercase();
    if error.contains("failed to get") && error.contains("write") && error.contains("lock") {
        Some(CodigoFalloArranque::BloqueoEscrituraRecurso)
    } else if error.contains("permission denied") || error.contains("permiso denegado") {
        Some(CodigoFalloArranque::PermisoRecursoDenegado)
    } else if error.contains("network")
        && (error.contains("not active") || error.contains("inactive"))
    {
        Some(CodigoFalloArranque::RedRequeridaInactiva)
    } else if error.contains("no such file")
        || error.contains("not found")
        || error.contains("failed to open")
    {
        Some(CodigoFalloArranque::RecursoRequeridoAusente)
    } else if error.contains("swtpm") || error.contains(" tpm") {
        Some(CodigoFalloArranque::TpmNoDisponible)
    } else if error.contains("nvram")
        || error.contains("firmware")
        || error.contains("uefi")
        || error.contains("ovmf")
    {
        Some(CodigoFalloArranque::FirmwareNoDisponible)
    } else if error.contains("no space left") || error.contains("disk full") {
        Some(CodigoFalloArranque::EspacioInsuficiente)
    } else if error.contains("unsupported")
        || error.contains("not supported")
        || error.contains("invalid argument")
    {
        Some(CodigoFalloArranque::ConfiguracionNoAdmitida)
    } else {
        None
    }
}

fn etiqueta_fallo_arranque(codigo: CodigoFalloArranque) -> &'static str {
    match codigo {
        CodigoFalloArranque::BloqueoEscrituraRecurso => "bloqueo_escritura_recurso",
        CodigoFalloArranque::PermisoRecursoDenegado => "permiso_recurso_denegado",
        CodigoFalloArranque::RecursoRequeridoAusente => "recurso_requerido_ausente",
        CodigoFalloArranque::RedRequeridaInactiva => "red_requerida_inactiva",
        CodigoFalloArranque::FirmwareNoDisponible => "firmware_no_disponible",
        CodigoFalloArranque::TpmNoDisponible => "tpm_no_disponible",
        CodigoFalloArranque::EspacioInsuficiente => "espacio_insuficiente",
        CodigoFalloArranque::ConfiguracionNoAdmitida => "configuracion_no_admitida",
        CodigoFalloArranque::NoClasificado => "fallo_no_clasificado",
    }
}

fn extraer_direccion(salida: &str) -> Option<IpAddr> {
    let mut candidatas = salida.lines().filter_map(|linea| {
        let columnas = linea.split_whitespace().collect::<Vec<_>>();
        if columnas.len() < 4 || !matches!(columnas[columnas.len() - 2], "ipv4" | "ipv6") {
            return None;
        }
        columnas
            .last()?
            .split('/')
            .next()?
            .parse::<IpAddr>()
            .ok()
            .filter(|direccion| {
                !direccion.is_unspecified() && !direccion.is_loopback() && !direccion.is_multicast()
            })
    });
    candidatas
        .clone()
        .find(IpAddr::is_ipv4)
        .or_else(|| candidatas.next())
}

fn volumen_en_listado(salida: &str, nombre: &str) -> bool {
    salida.lines().any(|linea| {
        linea
            .split_whitespace()
            .next()
            .is_some_and(|primera| primera == nombre)
    })
}

#[cfg(test)]
mod pruebas {
    use super::*;

    const XML: &str = r#"
<domain type='kvm'>
  <name>base</name><uuid>11111111-1111-1111-1111-111111111111</uuid>
  <os><type>hvm</type><nvram template='/firmware'>/estado/base.fd</nvram></os>
  <devices>
    <disk type='file' device='disk'><driver type='qcow2'/><source file='/base.qcow2'/><target dev='sda'/></disk>
    <disk type='file' device='cdrom'><source file='/entrada.iso'/><target dev='sdb'/><readonly/></disk>
    <interface type='network'><mac address='52:54:00:00:00:01'/><source network='default'/></interface>
    <filesystem type='mount'><source dir='/host'/><target dir='host'/></filesystem>
    <hostdev mode='subsystem'/><redirdev bus='usb' type='spicevmc'/>
    <channel type='spicevmc'><target name='com.redhat.spice.0'/></channel>
    <graphics type='spice'><clipboard copypaste='yes'/></graphics>
  </devices>
</domain>"#;

    #[test]
    fn sanea_un_clon_con_red_sin_compartir_recursos() {
        let mut dominio = Element::parse(XML.as_bytes()).unwrap();
        sanear_clon(
            &mut dominio,
            "lab-prueba-uno",
            "/instancia.qcow2",
            Some("aislada"),
        )
        .unwrap();
        assert!(dominio.get_child("uuid").is_none());
        let dispositivos = dominio.get_child("devices").unwrap();
        assert!(!dispositivos.children.iter().any(|nodo| {
            matches!(nodo, XMLNode::Element(elemento)
                if matches!(elemento.name.as_str(), "filesystem" | "hostdev" | "redirdev" | "channel"))
        }));
        assert_eq!(discos_dominio(&dominio).unwrap().len(), 1);
    }

    #[test]
    fn elimina_todas_las_interfaces_en_una_plantilla_sin_red() {
        let mut dominio = Element::parse(XML.as_bytes()).unwrap();
        sanear_clon(&mut dominio, "lab-prueba-dos", "/instancia.qcow2", None).unwrap();
        let dispositivos = dominio.get_child("devices").unwrap();
        assert!(!dispositivos.children.iter().any(
            |nodo| matches!(nodo, XMLNode::Element(elemento) if elemento.name == "interface")
        ));
        validar_clon_definido_sin_plantilla(&dominio, "lab-prueba-dos", "/instancia.qcow2", None)
            .unwrap();
    }

    #[test]
    fn detecta_identidades_reutilizadas() {
        let huella = HuellaIdentidad {
            uuid: "uno".to_owned(),
            nvram: Some("/estado/uno".to_owned()),
            tpm: None,
            macs: Vec::new(),
        };
        assert!(identidad_reutilizada(&huella.nvram, &huella.nvram));
        assert!(!identidad_reutilizada(&huella.tpm, &huella.tpm));
    }

    #[test]
    fn extrae_una_direccion_de_una_concesion_libvirt() {
        let salida =
            " Name MAC address Protocol Address\n vnet0 52:54:00:00:00:01 ipv4 192.0.2.25/24\n";
        assert_eq!(
            extraer_direccion(salida),
            Some("192.0.2.25".parse().unwrap())
        );
    }

    #[test]
    fn localiza_un_volumen_por_su_primera_columna_exacta() {
        let salida =
            " Name Path\n--------------------------------\n lab-uno.qcow2 /pool/lab-uno.qcow2\n";
        assert!(volumen_en_listado(salida, "lab-uno.qcow2"));
        assert!(!volumen_en_listado(salida, "lab-uno"));
    }

    #[test]
    fn clasifica_un_bloqueo_de_escritura_sin_publicar_el_error() {
        let error = "qemu: Failed to get 'write' lock\nIs another process using the image?";
        assert_eq!(
            clasificar_fallo_arranque(error),
            Some(CodigoFalloArranque::BloqueoEscrituraRecurso)
        );
        assert_eq!(
            etiqueta_fallo_arranque(CodigoFalloArranque::BloqueoEscrituraRecurso),
            "bloqueo_escritura_recurso"
        );
    }

    #[test]
    fn detecta_un_backing_bloqueado_por_otro_dominio_en_escritura() {
        let volumen = Element::parse(
            br#"<volume><backingStore><path>/privado/base.qcow2</path><format type='qcow2'/></backingStore></volume>"#
                .as_slice(),
        )
        .unwrap();
        let activo = Element::parse(
            br#"<domain><os><nvram>/privado/otro.fd</nvram></os><devices><disk device='disk'><source file='/privado/base.qcow2'/><target dev='sda'/></disk></devices></domain>"#
                .as_slice(),
        )
        .unwrap();
        let backing = rutas_backing_volumen(&volumen).unwrap();
        let escritura = recursos_escritura_dominio(&activo).unwrap();
        assert!(backing.iter().any(|ruta| escritura.contains(ruta)));

        let activo_solo_lectura = Element::parse(
            br#"<domain><devices><disk device='disk'><source file='/privado/base.qcow2'/><target dev='sda'/><readonly/></disk></devices></domain>"#
                .as_slice(),
        )
        .unwrap();
        let escritura = recursos_escritura_dominio(&activo_solo_lectura).unwrap();
        assert!(backing.iter().all(|ruta| !escritura.contains(ruta)));
    }

    #[test]
    fn sanea_solo_medios_y_red_inventariados() {
        let mut dominio = Element::parse(XML.as_bytes()).unwrap();
        establecer_texto_hijo(&mut dominio, "name", "lab-candidata").unwrap();
        let medios = BTreeMap::from([("sdb".to_owned(), "/entrada.iso".to_owned())]);
        sanear_candidata_xml(
            &mut dominio,
            "lab-candidata",
            "/base.qcow2",
            Some("default"),
            Some("aislada"),
            &medios,
        )
        .unwrap();
        assert!(red_dominio_conforme(&dominio, Some("aislada")));
        let dispositivos = dominio.get_child("devices").unwrap();
        assert!(!dispositivos.children.iter().any(|nodo| {
            matches!(nodo, XMLNode::Element(elemento)
                if elemento.name == "disk"
                    && elemento.attributes.get("device").map(String::as_str) == Some("cdrom"))
        }));
    }

    #[test]
    fn rechaza_un_medio_no_inventariado() {
        let mut dominio = Element::parse(XML.as_bytes()).unwrap();
        establecer_texto_hijo(&mut dominio, "name", "lab-candidata").unwrap();
        assert!(sanear_candidata_xml(
            &mut dominio,
            "lab-candidata",
            "/base.qcow2",
            Some("default"),
            Some("aislada"),
            &BTreeMap::new(),
        )
        .is_err());
    }

    #[test]
    fn rechaza_retirar_un_medio_temporal_con_escritura_habilitada() {
        let xml = XML.replace("<readonly/>", "");
        let mut dominio = Element::parse(xml.as_bytes()).unwrap();
        establecer_texto_hijo(&mut dominio, "name", "lab-candidata").unwrap();
        let medios = BTreeMap::from([("sdb".to_owned(), "/entrada.iso".to_owned())]);
        assert!(sanear_candidata_xml(
            &mut dominio,
            "lab-candidata",
            "/base.qcow2",
            Some("default"),
            Some("aislada"),
            &medios,
        )
        .is_err());
    }

    #[test]
    fn liga_la_identidad_ssh_al_dominio_y_valida_el_blob_ed25519() {
        let recibo = ReciboReserva::nuevo(
            Identificador::nuevo("ejecucion-identidad").unwrap(),
            Identificador::nuevo("windows-origen").unwrap(),
            crate::dominio::plantilla::SistemaInvitado::Windows,
            1,
        )
        .unwrap();
        let dominio = Element::parse(
            format!(
                "<domain><name>{}</name><uuid>22222222-2222-2222-2222-222222222222</uuid></domain>",
                recibo.id_instancia
            )
            .as_bytes(),
        )
        .unwrap();
        let uuid = validar_dominio_de_reserva(&dominio, &recibo).unwrap();
        let dominio_ajeno = Element::parse(
            b"<domain><name>lab-otra</name><uuid>22222222-2222-2222-2222-222222222222</uuid></domain>"
                .as_slice(),
        )
        .unwrap();
        assert!(validar_dominio_de_reserva(&dominio_ajeno, &recibo).is_err());
        validar_marca_uuid(b"22222222-2222-2222-2222-222222222222\r\n", &uuid).unwrap();

        let mut blob = Vec::new();
        blob.extend_from_slice(&(b"ssh-ed25519".len() as u32).to_be_bytes());
        blob.extend_from_slice(b"ssh-ed25519");
        blob.extend_from_slice(&32_u32.to_be_bytes());
        let mut clave_ed25519 = [0x66_u8; 32];
        clave_ed25519[0] = 0x58;
        blob.extend_from_slice(&clave_ed25519);
        let clave = format!("ssh-ed25519 {} usuario@host\n", STANDARD.encode(&blob));
        let identidad = validar_clave_publica_openssh(clave.as_bytes()).unwrap();
        assert_eq!(identidad.algoritmo.como_str(), "ssh-ed25519");
        assert_eq!(
            identidad.clave_publica,
            concat!(
                "ssh-ed25519 ",
                "AAAAC3NzaC1lZDI1NTE5AAAAIFhmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZm"
            )
        );
        assert_eq!(
            identidad.huella_sha256,
            format!("SHA256:{}", STANDARD_NO_PAD.encode(Sha256::digest(&blob)))
        );
        assert!(validar_marca_uuid(b"33333333-3333-3333-3333-333333333333", &uuid).is_err());
        assert!(validar_clave_publica_openssh(
            format!("ssh-ed25519 {}", STANDARD.encode([7_u8; 32])).as_bytes()
        )
        .is_err());
        assert!(validar_clave_publica_openssh(
            format!("{} comentario-extra", clave.trim_end()).as_bytes()
        )
        .is_err());
    }

    #[test]
    fn rechaza_marcas_y_blobs_ssh_ambiguos_o_truncados() {
        let uuid = "22222222-2222-2222-2222-222222222222";
        assert!(validar_marca_uuid(format!("{uuid}\n\n").as_bytes(), uuid).is_err());
        assert!(validar_marca_uuid(b"22222222-2222-2222-2222-22222222222g", uuid).is_err());
        assert!(validar_marca_uuid(b"AAAAAAAA-AAAA-AAAA-AAAA-AAAAAAAAAAAA", uuid).is_err());

        let mut blob = Vec::new();
        blob.extend_from_slice(&11_u32.to_be_bytes());
        blob.extend_from_slice(b"ssh-ed25519");
        blob.extend_from_slice(&32_u32.to_be_bytes());
        blob.extend_from_slice(&[0x58_u8; 32]);
        let clave = |blob: &[u8]| format!("ssh-ed25519 {}", STANDARD.encode(blob));
        assert!(validar_clave_publica_openssh(clave(&blob[..blob.len() - 1]).as_bytes()).is_err());
        let mut sobrante = blob.clone();
        sobrante.push(0);
        assert!(validar_clave_publica_openssh(clave(&sobrante).as_bytes()).is_err());
        let mut longitud_imposible = blob.clone();
        longitud_imposible[15..19].copy_from_slice(&33_u32.to_be_bytes());
        assert!(validar_clave_publica_openssh(clave(&longitud_imposible).as_bytes()).is_err());
        assert!(validar_clave_publica_openssh(b"ssh-rsa AAAA").is_err());
        assert!(validar_clave_publica_openssh(b"ssh-ed25519 !!!!").is_err());
    }

    #[test]
    fn fija_las_rutas_de_identidad_por_perfil() {
        let windows = rutas_identidad_ssh(PerfilIdentidadAcceso::WindowsOpenssh);
        assert_eq!(
            windows.marca_uuid,
            r"C:\ProgramData\LaboratorioAplicaciones\estado\uuid-identidad-ssh.txt"
        );
        assert_eq!(
            windows.clave_publica,
            r"C:\ProgramData\ssh\ssh_host_ed25519_key.pub"
        );
        let linux = rutas_identidad_ssh(PerfilIdentidadAcceso::LinuxOpenssh);
        assert_eq!(
            linux.marca_uuid,
            "/var/lib/laboratorio-libvirt/identidad-ssh.uuid"
        );
        assert_eq!(linux.clave_publica, "/etc/ssh/ssh_host_ed25519_key.pub");
    }
}
