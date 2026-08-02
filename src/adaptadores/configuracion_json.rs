// Laboratorio de máquinas virtuales con libvirt
// Copyright (C) 2026 Alberto Avidad
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::aplicacion::puertos::CatalogoPlantillas;
use crate::dominio::identificador::Identificador;
use crate::dominio::plantilla::{CanalAcceso, Plantilla, PoliticaRed, SistemaInvitado};
use crate::dominio::reserva::CuotasResultados;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Read;
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

const MAXIMO_BYTES_CONFIGURACION: u64 = 1024 * 1024;

/// Configuración privada del anfitrión. Relaciona identificadores públicos con
/// recursos locales, pero nunca se serializa en respuestas de la CLI o la API.
#[derive(Debug, Clone)]
pub struct ConfiguracionLocal {
    pub uri_libvirt: String,
    pub raiz_recibos: PathBuf,
    pub raiz_resultados: PathBuf,
    pub api: Option<ConfiguracionApi>,
    pub idioma_predeterminado: String,
    pub directorio_idiomas: Option<PathBuf>,
    pub cuotas_resultados: CuotasResultados,
    pub maximo_reservas_activas: usize,
    plantillas: BTreeMap<Identificador, Plantilla>,
    definiciones_libvirt: BTreeMap<Identificador, DefinicionPlantillaLibvirt>,
}

#[derive(Debug, Clone)]
pub struct ConfiguracionApi {
    pub escucha: SocketAddr,
    pub fichero_token: PathBuf,
}

/// Datos exclusivos del adaptador libvirt. No forman parte del dominio ni del
/// contrato visible por los consumidores.
#[derive(Debug, Clone)]
pub struct DefinicionPlantillaLibvirt {
    pub dominio: String,
    pub uuid_esperado: String,
    pub destino_disco: String,
    pub pool_plantilla: String,
    pub volumen_plantilla: String,
    pub pool_instancias: String,
    pub red: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentoConfiguracion {
    version: u8,
    uri_libvirt: String,
    raiz_recibos: PathBuf,
    raiz_resultados: PathBuf,
    #[serde(default = "idioma_castellano")]
    idioma_predeterminado: String,
    #[serde(default)]
    directorio_idiomas: Option<PathBuf>,
    #[serde(default)]
    cuotas_resultados: CuotasResultados,
    #[serde(default = "maximo_reservas_predeterminado")]
    maximo_reservas_activas: usize,
    #[serde(default)]
    api: Option<ApiConfigurada>,
    plantillas: Vec<PlantillaConfigurada>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ApiConfigurada {
    escucha: String,
    fichero_token: PathBuf,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlantillaConfigurada {
    id: String,
    sistema: SistemaInvitado,
    politica_red: PoliticaRed,
    #[serde(default)]
    capacidades: Vec<String>,
    #[serde(default)]
    canal_acceso: Option<CanalAcceso>,
    dominio: String,
    uuid_esperado: String,
    destino_disco: String,
    pool_plantilla: String,
    volumen_plantilla: String,
    pool_instancias: String,
    #[serde(default)]
    red: Option<String>,
}

impl ConfiguracionLocal {
    pub fn cargar(ruta: &Path) -> Result<Self> {
        let metadatos = fs::symlink_metadata(ruta)
            .with_context(|| format!("no se pudo inspeccionar {}", ruta.display()))?;
        if metadatos.file_type().is_symlink() || !metadatos.is_file() {
            bail!("la configuración debe ser un fichero ordinario");
        }
        #[cfg(unix)]
        {
            if metadatos.mode() & 0o077 != 0 {
                bail!("la configuración permite acceso a grupo u otros usuarios");
            }
            // SAFETY: geteuid no recibe punteros ni tiene precondiciones.
            if metadatos.uid() != unsafe { libc::geteuid() } {
                bail!("la configuración no pertenece al usuario actual");
            }
        }
        if metadatos.len() > MAXIMO_BYTES_CONFIGURACION {
            bail!("la configuración supera la cuota de 1 MiB");
        }
        let mut contenido = Vec::with_capacity(metadatos.len() as usize);
        File::open(ruta)?.read_to_end(&mut contenido)?;
        let documento: DocumentoConfiguracion =
            serde_json::from_slice(&contenido).context("la configuración no es JSON válido")?;
        if documento.version != 1 {
            bail!("versión de configuración no compatible");
        }
        if documento.uri_libvirt != "qemu:///system" {
            bail!("solo se admite el hipervisor local qemu:///system");
        }
        validar_raiz(&documento.raiz_recibos, "recibos")?;
        validar_raiz(&documento.raiz_resultados, "resultados")?;
        if documento.raiz_recibos == documento.raiz_resultados {
            bail!("las raíces de recibos y resultados deben ser distintas");
        }
        validar_codigo_idioma(&documento.idioma_predeterminado)?;
        if let Some(directorio) = &documento.directorio_idiomas {
            validar_raiz(directorio, "catálogos de idioma")?;
        }
        if !documento.cuotas_resultados.es_valida() {
            bail!("las cuotas de resultados no son válidas");
        }
        if !(1..=128).contains(&documento.maximo_reservas_activas) {
            bail!("el máximo de reservas activas debe estar entre 1 y 128");
        }

        let api = documento.api.map(validar_api).transpose()?;
        let mut plantillas = BTreeMap::new();
        let mut definiciones_libvirt = BTreeMap::new();
        for configurada in documento.plantillas {
            let id = Identificador::nuevo(configurada.id)?;
            validar_nombre_libvirt(&configurada.dominio, "dominio")?;
            validar_uuid(&configurada.uuid_esperado)?;
            validar_nombre_libvirt(&configurada.destino_disco, "destino de disco")?;
            validar_nombre_libvirt(&configurada.pool_plantilla, "pool de plantilla")?;
            validar_nombre_libvirt(&configurada.volumen_plantilla, "volumen de plantilla")?;
            validar_nombre_libvirt(&configurada.pool_instancias, "pool de instancias")?;
            match (configurada.politica_red, configurada.red.as_deref()) {
                (PoliticaRed::SinRed, None) => {}
                (PoliticaRed::Aislada, Some(red)) => validar_nombre_libvirt(red, "red")?,
                (PoliticaRed::SinRed, Some(_)) => {
                    bail!("una plantilla sin red no puede declarar una red libvirt")
                }
                (PoliticaRed::Aislada, None) => {
                    bail!("una plantilla con red aislada debe declarar una red libvirt")
                }
            }
            if let Some(canal) = &configurada.canal_acceso {
                if configurada.politica_red == PoliticaRed::SinRed || canal.puerto == 0 {
                    bail!("un canal de acceso necesita una red y un puerto válidos");
                }
            }
            let capacidades = configurada
                .capacidades
                .into_iter()
                .map(Identificador::nuevo)
                .collect::<Result<BTreeSet<_>, _>>()?;
            let plantilla = Plantilla {
                id: id.clone(),
                sistema: configurada.sistema,
                politica_red: configurada.politica_red,
                canal_acceso: configurada.canal_acceso,
                capacidades,
            };
            let definicion = DefinicionPlantillaLibvirt {
                dominio: configurada.dominio,
                uuid_esperado: configurada.uuid_esperado.to_ascii_lowercase(),
                destino_disco: configurada.destino_disco,
                pool_plantilla: configurada.pool_plantilla,
                volumen_plantilla: configurada.volumen_plantilla,
                pool_instancias: configurada.pool_instancias,
                red: configurada.red,
            };
            if plantillas.insert(id.clone(), plantilla).is_some()
                || definiciones_libvirt.insert(id, definicion).is_some()
            {
                bail!("identificador de plantilla repetido");
            }
        }
        if plantillas.is_empty() {
            bail!("la configuración no contiene plantillas registradas");
        }
        Ok(Self {
            uri_libvirt: documento.uri_libvirt,
            raiz_recibos: documento.raiz_recibos,
            raiz_resultados: documento.raiz_resultados,
            api,
            idioma_predeterminado: documento.idioma_predeterminado,
            directorio_idiomas: documento.directorio_idiomas,
            cuotas_resultados: documento.cuotas_resultados,
            maximo_reservas_activas: documento.maximo_reservas_activas,
            plantillas,
            definiciones_libvirt,
        })
    }

    pub fn definiciones_libvirt(&self) -> BTreeMap<Identificador, DefinicionPlantillaLibvirt> {
        self.definiciones_libvirt.clone()
    }
}

impl CatalogoPlantillas for ConfiguracionLocal {
    fn obtener(&self, id: &Identificador) -> Result<Plantilla> {
        self.plantillas
            .get(id)
            .cloned()
            .context("plantilla no registrada en la configuración local")
    }

    fn listar(&self) -> Result<Vec<Plantilla>> {
        Ok(self.plantillas.values().cloned().collect())
    }
}

fn validar_api(valor: ApiConfigurada) -> Result<ConfiguracionApi> {
    let escucha: SocketAddr = valor
        .escucha
        .parse()
        .context("la dirección de escucha de la API no es válida")?;
    if !escucha.ip().is_loopback() {
        bail!("la API integrada solo puede escuchar en la interfaz local");
    }
    validar_raiz(&valor.fichero_token, "fichero de autenticación")?;
    Ok(ConfiguracionApi {
        escucha,
        fichero_token: valor.fichero_token,
    })
}

fn idioma_castellano() -> String {
    "es".to_owned()
}

fn maximo_reservas_predeterminado() -> usize {
    4
}

fn validar_codigo_idioma(valor: &str) -> Result<()> {
    if !(2..=16).contains(&valor.len())
        || !valor
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
    {
        bail!("código de idioma no válido");
    }
    Ok(())
}

fn validar_raiz(ruta: &Path, nombre: &str) -> Result<()> {
    if !ruta.is_absolute()
        || ruta == Path::new("/")
        || ruta
            .components()
            .any(|componente| matches!(componente, Component::ParentDir))
        || ruta.components().count() < 3
    {
        bail!("la ruta de {nombre} no es absoluta y acotada");
    }
    Ok(())
}

fn validar_nombre_libvirt(valor: &str, campo: &str) -> Result<()> {
    if valor.is_empty()
        || valor.len() > 80
        || !valor
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        || !valor
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        bail!("{campo} no válido en la configuración local");
    }
    Ok(())
}

fn validar_uuid(valor: &str) -> Result<()> {
    if valor.len() != 36
        || !valor.bytes().enumerate().all(|(indice, byte)| {
            if matches!(indice, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
    {
        bail!("UUID no válido en la configuración local");
    }
    Ok(())
}

#[cfg(test)]
mod pruebas {
    use super::*;
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn escribir_configuracion(red: serde_json::Value) -> (tempfile::TempDir, PathBuf) {
        let temporal = tempfile::tempdir().unwrap();
        let ruta = temporal.path().join("config.local.json");
        let documento = serde_json::json!({
            "version": 1,
            "uri_libvirt": "qemu:///system",
            "raiz_recibos": "/var/lib/laboratorio-libvirt/recibos",
            "raiz_resultados": "/var/lib/laboratorio-libvirt/resultados",
            "idioma_predeterminado": "es",
            "plantillas": [{
                "id": "windows-analisis",
                "sistema": "windows",
                "politica_red": "aislada",
                "capacidades": ["analisis-windows"],
                "dominio": "win-base",
                "uuid_esperado": "11111111-1111-1111-1111-111111111111",
                "destino_disco": "sda",
                "pool_plantilla": "plantillas",
                "volumen_plantilla": "windows-base.qcow2",
                "pool_instancias": "instancias",
                "red": red
            }]
        });
        let mut archivo = File::create(&ruta).unwrap();
        write!(archivo, "{documento}").unwrap();
        drop(archivo);
        #[cfg(unix)]
        fs::set_permissions(&ruta, fs::Permissions::from_mode(0o600)).unwrap();
        (temporal, ruta)
    }

    #[test]
    fn separa_catalogo_publico_de_referencias_libvirt() {
        let (_temporal, ruta) = escribir_configuracion(serde_json::json!("lab-aislada"));
        let configuracion = ConfiguracionLocal::cargar(&ruta).unwrap();
        let id = Identificador::nuevo("windows-analisis").unwrap();
        let plantilla = configuracion.obtener(&id).unwrap();
        assert_eq!(plantilla.sistema, SistemaInvitado::Windows);
        assert!(plantilla
            .capacidades
            .contains(&Identificador::nuevo("analisis-windows").unwrap()));
        assert_eq!(
            configuracion
                .definiciones_libvirt()
                .get(&id)
                .unwrap()
                .dominio,
            "win-base"
        );
    }

    #[test]
    fn exige_coherencia_entre_politica_y_red() {
        let (_temporal, ruta) = escribir_configuracion(serde_json::Value::Null);
        assert!(ConfiguracionLocal::cargar(&ruta).is_err());
    }

    #[test]
    fn rechaza_una_api_expuesta_directamente() {
        let temporal = tempfile::tempdir().unwrap();
        let ruta = temporal.path().join("config.local.json");
        let documento = serde_json::json!({
            "version": 1,
            "uri_libvirt": "qemu:///system",
            "raiz_recibos": "/var/lib/laboratorio-libvirt/recibos",
            "raiz_resultados": "/var/lib/laboratorio-libvirt/resultados",
            "api": {"escucha": "0.0.0.0:8787", "fichero_token": "/var/lib/laboratorio-libvirt/token"},
            "plantillas": [{
                "id": "linux-pruebas", "sistema": "linux", "politica_red": "sin_red",
                "dominio": "linux-base", "uuid_esperado": "11111111-1111-1111-1111-111111111111",
                "destino_disco": "vda", "pool_plantilla": "plantillas",
                "volumen_plantilla": "linux-base.qcow2", "pool_instancias": "instancias"
            }]
        });
        serde_json::to_writer(&mut File::create(&ruta).unwrap(), &documento).unwrap();
        #[cfg(unix)]
        fs::set_permissions(&ruta, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(ConfiguracionLocal::cargar(&ruta).is_err());
    }
}
