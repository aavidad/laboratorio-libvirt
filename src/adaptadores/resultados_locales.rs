// Laboratorio de máquinas virtuales con libvirt
// Copyright (C) 2026 Alberto Avidad
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::aplicacion::puertos::VerificadorResultados;
use crate::dominio::identificador::Identificador;
use crate::dominio::reserva::{CuotasResultados, ResultadosProtegidos};
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

const NOMBRE_MANIFIESTO: &str = "manifiesto-laboratorio.json";
pub struct VerificadorResultadosLocales {
    raiz: PathBuf,
    cuotas: CuotasResultados,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifiesto {
    version: u8,
    id_ejecucion: Identificador,
    artefactos: Vec<Artefacto>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Artefacto {
    ruta: String,
    tamano_bytes: u64,
    sha256: String,
}

impl VerificadorResultadosLocales {
    pub fn nuevo(raiz: &Path) -> Result<Self> {
        Self::con_cuotas(raiz, CuotasResultados::default())
    }

    pub fn con_cuotas(raiz: &Path, cuotas: CuotasResultados) -> Result<Self> {
        validar_directorio_privado(raiz)?;
        if !cuotas.es_valida() {
            bail!("las cuotas de resultados no son válidas");
        }
        Ok(Self {
            raiz: raiz.to_path_buf(),
            cuotas,
        })
    }
}

impl VerificadorResultados for VerificadorResultadosLocales {
    fn verificar(&self, id_ejecucion: &Identificador) -> Result<ResultadosProtegidos> {
        let directorio = self.raiz.join(id_ejecucion.como_str());
        validar_directorio_privado(&directorio)?;
        let ruta_manifiesto = directorio.join(NOMBRE_MANIFIESTO);
        validar_fichero_ordinario(&ruta_manifiesto)?;
        if fs::metadata(&ruta_manifiesto)?.len() > self.cuotas.maximo_bytes_manifiesto {
            bail!("el manifiesto supera la cuota");
        }
        let manifiesto: Manifiesto = serde_json::from_reader(File::open(&ruta_manifiesto)?)
            .context("el manifiesto de resultados no es JSON válido")?;
        if manifiesto.version != 1 || &manifiesto.id_ejecucion != id_ejecucion {
            bail!("el manifiesto no corresponde a la ejecución");
        }
        if manifiesto.artefactos.is_empty()
            || manifiesto.artefactos.len() > self.cuotas.maximo_artefactos
        {
            bail!("cantidad de artefactos fuera de cuota");
        }

        let mut rutas = BTreeSet::new();
        let mut directorios_esperados = BTreeSet::new();
        let mut bytes = 0_u64;
        for artefacto in &manifiesto.artefactos {
            let relativa = Path::new(&artefacto.ruta);
            validar_ruta_relativa(relativa, &artefacto.ruta)?;
            if artefacto.ruta == NOMBRE_MANIFIESTO || !rutas.insert(artefacto.ruta.clone()) {
                bail!("ruta repetida o reservada en el manifiesto");
            }
            registrar_directorios_padre(relativa, &mut directorios_esperados)?;
            validar_componentes(&directorio, relativa)?;
            let ruta = directorio.join(relativa);
            validar_fichero_ordinario(&ruta)
                .with_context(|| format!("artefacto no válido: {}", artefacto.ruta))?;
            let tamano = fs::metadata(&ruta)?.len();
            if tamano != artefacto.tamano_bytes {
                bail!("el tamaño de un artefacto no coincide");
            }
            bytes = bytes
                .checked_add(tamano)
                .context("desbordamiento al sumar resultados")?;
            if bytes > self.cuotas.maximo_bytes {
                bail!("los resultados superan la cuota total");
            }
            if !es_sha256(&artefacto.sha256)
                || resumir_sha256(&ruta)? != artefacto.sha256.to_ascii_lowercase()
            {
                bail!("el SHA-256 de un artefacto no coincide");
            }
        }

        let inventario = inventariar(&directorio, self.cuotas.maximo_artefactos)?;
        let mut ficheros_esperados = rutas;
        ficheros_esperados.insert(NOMBRE_MANIFIESTO.to_owned());
        if inventario.ficheros != ficheros_esperados
            || inventario.directorios != directorios_esperados
        {
            bail!("la carpeta de resultados contiene entradas no declaradas");
        }
        Ok(ResultadosProtegidos {
            sha256_manifiesto: resumir_sha256(&ruta_manifiesto)?,
            artefactos: manifiesto.artefactos.len() as u64,
            bytes,
        })
    }
}

fn validar_ruta_relativa(ruta: &Path, original: &str) -> Result<()> {
    if original.is_empty()
        || ruta.is_absolute()
        || ruta
            .components()
            .any(|componente| !matches!(componente, Component::Normal(_)))
    {
        bail!("ruta no segura en el manifiesto");
    }
    Ok(())
}

fn registrar_directorios_padre(ruta: &Path, directorios: &mut BTreeSet<String>) -> Result<()> {
    let mut actual = PathBuf::new();
    if let Some(padre) = ruta.parent() {
        for componente in padre.components() {
            let Component::Normal(nombre) = componente else {
                bail!("ruta no segura en el manifiesto");
            };
            actual.push(nombre);
            directorios.insert(ruta_portable(&actual)?);
        }
    }
    Ok(())
}

fn validar_componentes(raiz: &Path, relativa: &Path) -> Result<()> {
    let componentes = relativa.components().collect::<Vec<_>>();
    let mut actual = raiz.to_path_buf();
    for (indice, componente) in componentes.iter().enumerate() {
        let Component::Normal(nombre) = componente else {
            bail!("componente no seguro en una ruta de resultados");
        };
        actual.push(nombre);
        let metadatos = fs::symlink_metadata(&actual)?;
        if metadatos.file_type().is_symlink() {
            bail!("los resultados contienen un enlace simbólico");
        }
        if indice + 1 < componentes.len() && !metadatos.is_dir() {
            bail!("un componente intermedio no es un directorio");
        }
    }
    Ok(())
}

struct Inventario {
    ficheros: BTreeSet<String>,
    directorios: BTreeSet<String>,
}

fn inventariar(raiz: &Path, maximo_artefactos: usize) -> Result<Inventario> {
    let mut pendientes = vec![(raiz.to_path_buf(), PathBuf::new())];
    let mut ficheros = BTreeSet::new();
    let mut directorios = BTreeSet::new();
    let mut entradas = 0_usize;
    while let Some((directorio, prefijo)) = pendientes.pop() {
        for entrada in fs::read_dir(&directorio)? {
            let entrada = entrada?;
            entradas = entradas.saturating_add(1);
            if entradas > maximo_artefactos.saturating_mul(2).saturating_add(1) {
                bail!("la estructura de resultados supera la cuota");
            }
            let metadatos = fs::symlink_metadata(entrada.path())?;
            if metadatos.file_type().is_symlink() {
                bail!("los resultados contienen un enlace simbólico");
            }
            let relativa = prefijo.join(entrada.file_name());
            if metadatos.is_dir() {
                directorios.insert(ruta_portable(&relativa)?);
                pendientes.push((entrada.path(), relativa));
            } else if metadatos.is_file() {
                validar_fichero_ordinario(&entrada.path())?;
                ficheros.insert(ruta_portable(&relativa)?);
            } else {
                bail!("los resultados contienen una entrada no ordinaria");
            }
        }
    }
    Ok(Inventario {
        ficheros,
        directorios,
    })
}

fn ruta_portable(ruta: &Path) -> Result<String> {
    Ok(ruta
        .to_str()
        .context("una ruta de resultados no es UTF-8")?
        .replace(std::path::MAIN_SEPARATOR, "/"))
}

fn validar_directorio(ruta: &Path) -> Result<()> {
    let metadatos = fs::symlink_metadata(ruta)?;
    if metadatos.file_type().is_symlink() || !metadatos.is_dir() {
        bail!("los resultados no residen en un directorio ordinario");
    }
    Ok(())
}

fn validar_directorio_privado(ruta: &Path) -> Result<()> {
    validar_directorio(ruta)?;
    #[cfg(unix)]
    {
        let metadatos = fs::metadata(ruta)?;
        if metadatos.mode() & 0o077 != 0 {
            bail!("el directorio de resultados permite acceso a grupo u otros usuarios");
        }
        // SAFETY: geteuid no recibe punteros ni tiene precondiciones.
        if metadatos.uid() != unsafe { libc::geteuid() } {
            bail!("el directorio de resultados no pertenece al usuario actual");
        }
    }
    Ok(())
}

fn validar_fichero_ordinario(ruta: &Path) -> Result<()> {
    let metadatos = fs::symlink_metadata(ruta)?;
    if metadatos.file_type().is_symlink() || !metadatos.is_file() {
        bail!("la entrada no es un fichero ordinario");
    }
    #[cfg(unix)]
    if metadatos.nlink() != 1 {
        bail!("los resultados contienen un enlace físico");
    }
    Ok(())
}

fn es_sha256(valor: &str) -> bool {
    valor.len() == 64 && valor.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn resumir_sha256(ruta: &Path) -> Result<String> {
    let mut archivo = File::open(ruta)?;
    let mut resumen = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let leidos = archivo.read(&mut buffer)?;
        if leidos == 0 {
            break;
        }
        resumen.update(&buffer[..leidos]);
    }
    Ok(format!("{:x}", resumen.finalize()))
}

#[cfg(test)]
mod pruebas {
    use super::*;
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn preparar_resultado() -> (tempfile::TempDir, Identificador, PathBuf) {
        let temporal = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        fs::set_permissions(temporal.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let id = Identificador::nuevo("ejecucion-resultados").unwrap();
        let directorio = temporal.path().join(id.como_str());
        fs::create_dir(&directorio).unwrap();
        #[cfg(unix)]
        fs::set_permissions(&directorio, fs::Permissions::from_mode(0o700)).unwrap();
        let artefacto = directorio.join("resumen.json");
        File::create(&artefacto)
            .unwrap()
            .write_all(b"original")
            .unwrap();
        let manifiesto = serde_json::json!({
            "version": 1,
            "id_ejecucion": id,
            "artefactos": [{
                "ruta": "resumen.json",
                "tamano_bytes": 8,
                "sha256": resumir_sha256(&artefacto).unwrap()
            }]
        });
        serde_json::to_writer_pretty(
            File::create(directorio.join(NOMBRE_MANIFIESTO)).unwrap(),
            &manifiesto,
        )
        .unwrap();
        (temporal, id, artefacto)
    }

    #[test]
    fn acepta_resultados_exactos() {
        let (temporal, id, _) = preparar_resultado();
        let verificador = VerificadorResultadosLocales::nuevo(temporal.path()).unwrap();
        let resultado = verificador.verificar(&id).unwrap();
        assert_eq!(resultado.artefactos, 1);
        assert_eq!(resultado.bytes, 8);
    }

    #[test]
    fn rechaza_un_artefacto_modificado() {
        let (temporal, id, artefacto) = preparar_resultado();
        File::create(&artefacto)
            .unwrap()
            .write_all(b"alterado!")
            .unwrap();
        let verificador = VerificadorResultadosLocales::nuevo(temporal.path()).unwrap();
        assert!(verificador.verificar(&id).is_err());
    }

    #[test]
    fn rechaza_un_directorio_adicional() {
        let (temporal, id, _) = preparar_resultado();
        fs::create_dir(temporal.path().join(id.como_str()).join("inesperado")).unwrap();
        let verificador = VerificadorResultadosLocales::nuevo(temporal.path()).unwrap();
        assert!(verificador.verificar(&id).is_err());
    }
}
