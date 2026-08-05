// Laboratorio de máquinas virtuales con libvirt
// Copyright (C) 2026 Alberto Avidad
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::aplicacion::puertos::{CatalogoDestinosPromocion, CatalogoPlantillas};
use crate::dominio::identificador::Identificador;
use crate::dominio::plantilla::{CanalAcceso, Plantilla, PoliticaRed, SistemaInvitado};
use crate::dominio::preparacion_plantilla::DestinoPromocion;
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
    destinos_promocion: BTreeMap<Identificador, DestinoPromocion>,
    definiciones_promocion: BTreeMap<Identificador, DefinicionPromocionLibvirt>,
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
    pub perfil_identidad_acceso: Option<PerfilIdentidadAcceso>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PerfilIdentidadAcceso {
    WindowsOpenssh,
    LinuxOpenssh,
}

#[derive(Debug, Clone)]
pub struct MedioTemporalLibvirt {
    pub destino: String,
    pub pool: String,
    pub volumen: String,
}

#[derive(Debug, Clone)]
pub struct DefinicionPromocionLibvirt {
    pub id_origen: Identificador,
    pub dominio_destino: String,
    pub uuid_destino: String,
    pub destino_disco: String,
    pub pool_destino: String,
    pub volumen_destino: String,
    pub red_preparacion: Option<String>,
    pub red_final: Option<String>,
    pub medios_temporales: Vec<MedioTemporalLibvirt>,
    pub perfil_identidad_acceso: Option<PerfilIdentidadAcceso>,
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
    #[serde(default)]
    promociones: Vec<PromocionConfigurada>,
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
    #[serde(default)]
    perfil_identidad_acceso: Option<PerfilIdentidadAcceso>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MedioTemporalConfigurado {
    destino: String,
    pool: String,
    volumen: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PromocionConfigurada {
    id: String,
    id_origen: String,
    sistema: SistemaInvitado,
    politica_red: PoliticaRed,
    #[serde(default)]
    capacidades: Vec<String>,
    #[serde(default)]
    canal_acceso: Option<CanalAcceso>,
    dominio_destino: String,
    uuid_destino: String,
    destino_disco: String,
    pool_destino: String,
    volumen_destino: String,
    #[serde(default)]
    red_preparacion: Option<String>,
    #[serde(default)]
    red_final: Option<String>,
    #[serde(default)]
    medios_temporales: Vec<MedioTemporalConfigurado>,
    #[serde(default)]
    perfil_identidad_acceso: Option<PerfilIdentidadAcceso>,
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
        if documento.version != 2 {
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
            match (
                configurada
                    .canal_acceso
                    .as_ref()
                    .map(|canal| canal.protocolo),
                configurada.perfil_identidad_acceso,
            ) {
                (
                    Some(crate::dominio::plantilla::ProtocoloAcceso::Ssh),
                    Some(PerfilIdentidadAcceso::WindowsOpenssh),
                ) if configurada.sistema == SistemaInvitado::Windows => {}
                (
                    Some(crate::dominio::plantilla::ProtocoloAcceso::Ssh),
                    Some(PerfilIdentidadAcceso::LinuxOpenssh),
                ) if configurada.sistema == SistemaInvitado::Linux => {}
                (Some(crate::dominio::plantilla::ProtocoloAcceso::Ssh), Some(_)) => {
                    bail!("el perfil de identidad no corresponde al sistema de la plantilla")
                }
                (Some(crate::dominio::plantilla::ProtocoloAcceso::Ssh), None) => {
                    bail!("un canal SSH exige un perfil de identidad fuera de banda")
                }
                (Some(_), Some(_)) | (None, Some(_)) => {
                    bail!("el perfil de identidad configurado solo es válido para SSH")
                }
                _ => {}
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
                perfil_identidad_acceso: configurada.perfil_identidad_acceso,
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
        let mut destinos_promocion = BTreeMap::new();
        let mut definiciones_promocion = BTreeMap::new();
        let mut dominios_destino = BTreeSet::new();
        let mut uuid_destino = BTreeSet::new();
        let mut volumenes_destino = BTreeSet::new();
        for configurada in documento.promociones {
            let id = Identificador::nuevo(configurada.id)?;
            let id_origen = Identificador::nuevo(configurada.id_origen)?;
            let origen = plantillas
                .get(&id_origen)
                .context("el origen de promoción no está registrado")?;
            if plantillas.contains_key(&id) {
                bail!("un destino de promoción no puede sobrescribir una plantilla existente");
            }
            if origen.sistema != configurada.sistema {
                bail!("el destino de promoción debe conservar la familia del sistema");
            }
            validar_nombre_libvirt(&configurada.dominio_destino, "dominio de promoción")?;
            validar_uuid(&configurada.uuid_destino)?;
            validar_nombre_libvirt(&configurada.destino_disco, "destino de disco")?;
            validar_nombre_libvirt(&configurada.pool_destino, "pool de promoción")?;
            validar_nombre_libvirt(&configurada.volumen_destino, "volumen de promoción")?;
            if configurada.dominio_destino.starts_with("lab-")
                || configurada.volumen_destino.starts_with("lab-")
            {
                bail!("un destino de promoción invade el espacio de nombres de reservas");
            }
            if definiciones_libvirt.values().any(|existente| {
                existente.dominio == configurada.dominio_destino
                    || existente
                        .uuid_esperado
                        .eq_ignore_ascii_case(&configurada.uuid_destino)
                    || (existente.pool_plantilla == configurada.pool_destino
                        && existente.volumen_plantilla == configurada.volumen_destino)
            }) {
                bail!("un destino de promoción sobrescribiría una plantilla registrada");
            }
            if !dominios_destino.insert(configurada.dominio_destino.clone())
                || !uuid_destino.insert(configurada.uuid_destino.to_ascii_lowercase())
                || !volumenes_destino.insert((
                    configurada.pool_destino.clone(),
                    configurada.volumen_destino.clone(),
                ))
            {
                bail!("dos destinos de promoción comparten un recurso privado");
            }
            if let Some(red) = &configurada.red_preparacion {
                validar_nombre_libvirt(red, "red temporal de preparación")?;
            }
            match (configurada.politica_red, configurada.red_final.as_deref()) {
                (PoliticaRed::SinRed, None) => {}
                (PoliticaRed::Aislada, Some(red)) => validar_nombre_libvirt(red, "red final")?,
                _ => bail!("la red final no coincide con la política del destino"),
            }
            if let Some(canal) = &configurada.canal_acceso {
                if configurada.politica_red == PoliticaRed::SinRed || canal.puerto == 0 {
                    bail!("un destino sin red no puede declarar canal de acceso");
                }
            }
            match (
                configurada
                    .canal_acceso
                    .as_ref()
                    .map(|canal| canal.protocolo),
                configurada.perfil_identidad_acceso,
            ) {
                (
                    Some(crate::dominio::plantilla::ProtocoloAcceso::Ssh),
                    Some(PerfilIdentidadAcceso::WindowsOpenssh),
                ) if configurada.sistema == SistemaInvitado::Windows => {}
                (
                    Some(crate::dominio::plantilla::ProtocoloAcceso::Ssh),
                    Some(PerfilIdentidadAcceso::LinuxOpenssh),
                ) if configurada.sistema == SistemaInvitado::Linux => {}
                (Some(crate::dominio::plantilla::ProtocoloAcceso::Ssh), Some(_)) => {
                    bail!("el perfil de identidad no corresponde al sistema del destino")
                }
                (Some(crate::dominio::plantilla::ProtocoloAcceso::Ssh), None) => {
                    bail!("un destino SSH exige un perfil de identidad fuera de banda")
                }
                (Some(_), Some(_)) | (None, Some(_)) => {
                    bail!("el perfil de identidad del destino solo es válido para SSH")
                }
                _ => {}
            }
            let capacidades = configurada
                .capacidades
                .into_iter()
                .map(Identificador::nuevo)
                .collect::<Result<BTreeSet<_>, _>>()?;
            let destino_disco_sistema = configurada.destino_disco.clone();
            let mut destinos_medios = BTreeSet::new();
            let mut recursos_medios = BTreeSet::new();
            let medios_temporales = configurada
                .medios_temporales
                .into_iter()
                .map(|medio| {
                    validar_nombre_libvirt(&medio.destino, "destino de medio temporal")?;
                    validar_nombre_libvirt(&medio.pool, "pool de medio temporal")?;
                    validar_nombre_libvirt(&medio.volumen, "volumen de medio temporal")?;
                    if medio.destino == destino_disco_sistema {
                        bail!("un medio temporal no puede ocupar el destino del disco sistema");
                    }
                    if !destinos_medios.insert(medio.destino.clone()) {
                        bail!("destino de medio temporal repetido");
                    }
                    if !recursos_medios.insert((medio.pool.clone(), medio.volumen.clone())) {
                        bail!("recurso de medio temporal repetido");
                    }
                    Ok(MedioTemporalLibvirt {
                        destino: medio.destino,
                        pool: medio.pool,
                        volumen: medio.volumen,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            let destino = DestinoPromocion {
                plantilla: Plantilla {
                    id: id.clone(),
                    sistema: configurada.sistema,
                    politica_red: configurada.politica_red,
                    canal_acceso: configurada.canal_acceso,
                    capacidades,
                },
                id_origen: id_origen.clone(),
            };
            let definicion = DefinicionPromocionLibvirt {
                id_origen,
                dominio_destino: configurada.dominio_destino,
                uuid_destino: configurada.uuid_destino.to_ascii_lowercase(),
                destino_disco: configurada.destino_disco,
                pool_destino: configurada.pool_destino,
                volumen_destino: configurada.volumen_destino,
                red_preparacion: configurada.red_preparacion,
                red_final: configurada.red_final,
                medios_temporales,
                perfil_identidad_acceso: configurada.perfil_identidad_acceso,
            };
            if destinos_promocion.insert(id.clone(), destino).is_some()
                || definiciones_promocion.insert(id, definicion).is_some()
            {
                bail!("identificador de destino de promoción repetido");
            }
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
            destinos_promocion,
            definiciones_promocion,
        })
    }

    pub fn definiciones_libvirt(&self) -> BTreeMap<Identificador, DefinicionPlantillaLibvirt> {
        self.definiciones_libvirt.clone()
    }

    pub fn definiciones_promocion(&self) -> BTreeMap<Identificador, DefinicionPromocionLibvirt> {
        self.definiciones_promocion.clone()
    }
}

impl CatalogoDestinosPromocion for ConfiguracionLocal {
    fn obtener_destino(&self, id: &Identificador) -> Result<DestinoPromocion> {
        self.destinos_promocion
            .get(id)
            .cloned()
            .context("destino de promoción no registrado en la configuración local")
    }

    fn listar_destinos(&self) -> Result<Vec<DestinoPromocion>> {
        Ok(self.destinos_promocion.values().cloned().collect())
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
                byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')
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
            "version": 2,
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
    fn exige_identidad_fuera_de_banda_para_ssh() {
        let (_temporal, ruta) = escribir_configuracion(serde_json::json!("lab-aislada"));
        let mut documento: serde_json::Value =
            serde_json::from_reader(File::open(&ruta).unwrap()).unwrap();
        documento["plantillas"][0]["canal_acceso"] = serde_json::json!({
            "protocolo": "ssh",
            "puerto": 22
        });
        serde_json::to_writer(&mut File::create(&ruta).unwrap(), &documento).unwrap();
        #[cfg(unix)]
        fs::set_permissions(&ruta, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(ConfiguracionLocal::cargar(&ruta).is_err());
    }

    #[test]
    fn rechaza_un_perfil_ssh_de_otro_sistema_en_una_plantilla() {
        let (_temporal, ruta) = escribir_configuracion(serde_json::json!("lab-aislada"));
        let mut documento: serde_json::Value =
            serde_json::from_reader(File::open(&ruta).unwrap()).unwrap();
        documento["plantillas"][0]["canal_acceso"] = serde_json::json!({
            "protocolo": "ssh",
            "puerto": 22
        });
        documento["plantillas"][0]["perfil_identidad_acceso"] = serde_json::json!("linux_openssh");
        serde_json::to_writer(&mut File::create(&ruta).unwrap(), &documento).unwrap();
        #[cfg(unix)]
        fs::set_permissions(&ruta, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(ConfiguracionLocal::cargar(&ruta).is_err());
    }

    #[test]
    fn rechaza_un_perfil_ssh_de_otro_sistema_en_un_destino() {
        let temporal = tempfile::tempdir().unwrap();
        let ruta = temporal.path().join("config.local.json");
        let mut documento: serde_json::Value =
            serde_json::from_str(include_str!("../../config.example.json")).unwrap();
        documento["promociones"][0]["perfil_identidad_acceso"] = serde_json::json!("linux_openssh");
        serde_json::to_writer(&mut File::create(&ruta).unwrap(), &documento).unwrap();
        #[cfg(unix)]
        fs::set_permissions(&ruta, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(ConfiguracionLocal::cargar(&ruta).is_err());
    }

    #[test]
    fn rechaza_medios_temporales_duplicados_o_sobre_el_disco_sistema() {
        for medio_adicional in [
            serde_json::json!({
                "destino": "sdb",
                "pool": "otro-pool",
                "volumen": "otro.iso"
            }),
            serde_json::json!({
                "destino": "sdc",
                "pool": "medios-preparacion",
                "volumen": "bootstrap-controlado.iso"
            }),
            serde_json::json!({
                "destino": "sda",
                "pool": "otro-pool",
                "volumen": "otro.iso"
            }),
        ] {
            let temporal = tempfile::tempdir().unwrap();
            let ruta = temporal.path().join("config.local.json");
            let mut documento: serde_json::Value =
                serde_json::from_str(include_str!("../../config.example.json")).unwrap();
            documento["promociones"][0]["medios_temporales"]
                .as_array_mut()
                .unwrap()
                .push(medio_adicional);
            serde_json::to_writer(&mut File::create(&ruta).unwrap(), &documento).unwrap();
            #[cfg(unix)]
            fs::set_permissions(&ruta, fs::Permissions::from_mode(0o600)).unwrap();
            assert!(ConfiguracionLocal::cargar(&ruta).is_err());
        }
    }

    #[test]
    fn rechaza_una_api_expuesta_directamente() {
        let temporal = tempfile::tempdir().unwrap();
        let ruta = temporal.path().join("config.local.json");
        let documento = serde_json::json!({
            "version": 2,
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

    #[test]
    fn exige_uuid_canonico_en_minusculas() {
        assert!(validar_uuid("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").is_ok());
        assert!(validar_uuid("AAAAAAAA-AAAA-AAAA-AAAA-AAAAAAAAAAAA").is_err());
    }

    #[test]
    fn rechaza_promover_sobre_una_plantilla_existente() {
        let temporal = tempfile::tempdir().unwrap();
        let ruta = temporal.path().join("config.local.json");
        let documento = serde_json::json!({
            "version": 2,
            "uri_libvirt": "qemu:///system",
            "raiz_recibos": "/var/lib/laboratorio-libvirt/recibos",
            "raiz_resultados": "/var/lib/laboratorio-libvirt/resultados",
            "plantillas": [{
                "id": "linux-base", "sistema": "linux", "politica_red": "sin_red",
                "dominio": "linux-base", "uuid_esperado": "11111111-1111-1111-1111-111111111111",
                "destino_disco": "vda", "pool_plantilla": "plantillas",
                "volumen_plantilla": "linux-base.qcow2", "pool_instancias": "instancias"
            }],
            "promociones": [{
                "id": "linux-base", "id_origen": "linux-base", "sistema": "linux",
                "politica_red": "sin_red", "dominio_destino": "linux-nueva",
                "uuid_destino": "22222222-2222-2222-2222-222222222222",
                "destino_disco": "vda", "pool_destino": "plantillas",
                "volumen_destino": "linux-nueva.qcow2"
            }]
        });
        serde_json::to_writer(&mut File::create(&ruta).unwrap(), &documento).unwrap();
        #[cfg(unix)]
        fs::set_permissions(&ruta, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(ConfiguracionLocal::cargar(&ruta).is_err());
    }

    #[test]
    fn registra_un_destino_sin_exponer_sus_recursos() {
        let temporal = tempfile::tempdir().unwrap();
        let ruta = temporal.path().join("config.local.json");
        let documento = serde_json::json!({
            "version": 2,
            "uri_libvirt": "qemu:///system",
            "raiz_recibos": "/var/lib/laboratorio-libvirt/recibos",
            "raiz_resultados": "/var/lib/laboratorio-libvirt/resultados",
            "plantillas": [{
                "id": "linux-base", "sistema": "linux", "politica_red": "sin_red",
                "dominio": "linux-base", "uuid_esperado": "11111111-1111-1111-1111-111111111111",
                "destino_disco": "vda", "pool_plantilla": "plantillas",
                "volumen_plantilla": "linux-base.qcow2", "pool_instancias": "instancias"
            }],
            "promociones": [{
                "id": "linux-siguiente", "id_origen": "linux-base", "sistema": "linux",
                "politica_red": "sin_red", "dominio_destino": "linux-siguiente-base",
                "uuid_destino": "22222222-2222-2222-2222-222222222222",
                "destino_disco": "vda", "pool_destino": "plantillas",
                "volumen_destino": "linux-siguiente.qcow2"
            }]
        });
        serde_json::to_writer(&mut File::create(&ruta).unwrap(), &documento).unwrap();
        #[cfg(unix)]
        fs::set_permissions(&ruta, fs::Permissions::from_mode(0o600)).unwrap();
        let configuracion = ConfiguracionLocal::cargar(&ruta).unwrap();
        let destinos = configuracion.listar_destinos().unwrap();
        assert_eq!(destinos.len(), 1);
        let publico = serde_json::to_string(&destinos).unwrap();
        assert!(!publico.contains("linux-siguiente-base"));
        assert!(!publico.contains("qcow2"));
    }
}
