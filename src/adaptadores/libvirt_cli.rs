// Laboratorio de máquinas virtuales con libvirt
// Copyright (C) 2026 Alberto Avidad
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::adaptadores::configuracion_json::DefinicionPlantillaLibvirt;
use crate::aplicacion::puertos::{EstadoInstancia, ProveedorInstancias};
use crate::dominio::identificador::Identificador;
use crate::dominio::plantilla::{EstadoPlantilla, Plantilla, PoliticaRed};
use crate::dominio::reserva::ReciboReserva;
use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::IpAddr;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use wait_timeout::ChildExt;
use xmltree::{Element, XMLNode};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

const VIRSH: &str = "/usr/bin/virsh";
const PLAZO_COMANDO: Duration = Duration::from_secs(30);
const MAXIMO_BYTES_SALIDA: u64 = 16 * 1024 * 1024;

#[derive(Debug)]
struct SalidaComando {
    correcta: bool,
    salida: String,
    error: String,
}

trait EjecutorComandos: Send + Sync {
    fn ejecutar(&self, programa: &Path, argumentos: &[OsString]) -> Result<SalidaComando>;
}

struct EjecutorSistema;

impl EjecutorComandos for EjecutorSistema {
    fn ejecutar(&self, programa: &Path, argumentos: &[OsString]) -> Result<SalidaComando> {
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
        let estado = match hijo.wait_timeout(PLAZO_COMANDO)? {
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
    ejecutor: Box<dyn EjecutorComandos>,
}

impl LibvirtCli {
    pub fn nuevo(
        uri: impl Into<String>,
        definiciones: BTreeMap<Identificador, DefinicionPlantillaLibvirt>,
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
            ejecutor: Box::new(EjecutorSistema),
        })
    }

    fn definicion(&self, plantilla: &Plantilla) -> Result<&DefinicionPlantillaLibvirt> {
        self.definiciones
            .get(&plantilla.id)
            .context("la plantilla no tiene una definición libvirt registrada")
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
            .ejecutar(Path::new(VIRSH), &argumentos_finales)
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

    fn dumpxml(&self, dominio: &str) -> Result<Element> {
        let xml = self.virsh_correcto(
            ["dumpxml", "--inactive", dominio],
            "no se pudo leer la definición del dominio",
        )?;
        Element::parse(xml.as_bytes()).context("libvirt devolvió un XML de dominio no válido")
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
        let temporal = std::env::temp_dir().join(format!(
            "laboratorio-libvirt-{}-{}.xml",
            std::process::id(),
            id_instancia.como_str()
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
        let definicion_clon = self.virsh_correcto(
            ["define", ruta_utf8(&temporal)?],
            "no se pudo definir la instancia efímera",
        );
        fs::remove_file(&temporal).with_context(|| {
            format!("no se pudo retirar el XML temporal {}", temporal.display())
        })?;
        definicion_clon?;
        Ok(huella_plantilla)
    }

    fn retirar_definicion(&self, id_instancia: &Identificador, dominio: &Element) -> Result<()> {
        let mut argumentos = vec![
            OsString::from("undefine"),
            OsString::from(id_instancia.como_str()),
        ];
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
        if !salida.correcta && self.dominio_existe(id_instancia.como_str())? {
            bail!(
                "no se pudo retirar la definición exacta: {}",
                sanear_error(&salida.error)
            );
        }
        Ok(())
    }

    fn red_aislada_disponible(&self, nombre: &str) -> Result<bool> {
        let activas = self.virsh_correcto(
            ["net-list", "--name"],
            "no se pudieron enumerar las redes activas",
        )?;
        if !activas.lines().any(|linea| linea.trim() == nombre) {
            return Ok(false);
        }
        let xml = self.virsh_correcto(
            ["net-dumpxml", nombre],
            "no se pudo inspeccionar la red registrada",
        )?;
        let red = Element::parse(xml.as_bytes()).context("XML de red no válido")?;
        Ok(red.get_child("forward").is_none())
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
        self.virsh_correcto(
            ["start", id_instancia.como_str()],
            "no se pudo iniciar la instancia",
        )?;
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
        for origen in ["lease", "agent"] {
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
    <disk type='file' device='cdrom'><source file='/entrada.iso'/><target dev='sdb'/></disk>
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
}
