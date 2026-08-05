// Laboratorio de máquinas virtuales con libvirt
// Copyright (C) 2026 Alberto Avidad
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::aplicacion::puertos::{
    AlmacenRecibosReserva, GuardiaMutacion, GuardiaPreparacion, Reloj,
};
use crate::dominio::identificador::Identificador;
use crate::dominio::reserva::ReciboReserva;
use anyhow::{bail, Context, Result};
use fs2::FileExt;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};

static CONTADOR_TEMPORALES: AtomicU64 = AtomicU64::new(0);

pub struct AlmacenRecibosJson {
    raiz: PathBuf,
}

impl AlmacenRecibosJson {
    pub fn nuevo(raiz: &Path) -> Result<Self> {
        validar_directorio_privado(raiz)?;
        let almacen = Self {
            raiz: raiz.to_path_buf(),
        };
        almacen.asegurar_bloqueo_global()?;
        Ok(almacen)
    }

    fn directorio(&self, id_ejecucion: &Identificador) -> PathBuf {
        self.raiz.join(id_ejecucion.como_str())
    }

    fn ruta_recibo(&self, id_ejecucion: &Identificador) -> PathBuf {
        self.directorio(id_ejecucion).join("recibo.json")
    }

    fn abrir_bloqueo(&self, id_ejecucion: &Identificador) -> Result<File> {
        let ruta = self.directorio(id_ejecucion).join(".bloqueo");
        let metadatos = fs::symlink_metadata(&ruta)
            .with_context(|| format!("no se pudo inspeccionar {}", ruta.display()))?;
        if metadatos.file_type().is_symlink() || !metadatos.is_file() {
            bail!("el bloqueo de la reserva no es un fichero ordinario");
        }
        let archivo = OpenOptions::new().read(true).write(true).open(ruta)?;
        archivo.lock_exclusive()?;
        Ok(archivo)
    }

    fn abrir_bloqueo_operacion(&self, id_ejecucion: &Identificador) -> Result<File> {
        let ruta = self.directorio(id_ejecucion).join(".operacion");
        if !ruta.try_exists()? {
            let mut opciones = OpenOptions::new();
            opciones.write(true).create_new(true);
            #[cfg(unix)]
            opciones.mode(0o600);
            if let Err(error) = opciones.open(&ruta) {
                if !ruta.try_exists()? {
                    return Err(error).context("no se pudo crear el bloqueo de operación");
                }
            }
        }
        validar_fichero_privado(&ruta)?;
        let archivo = OpenOptions::new().read(true).write(true).open(ruta)?;
        archivo.lock_exclusive()?;
        Ok(archivo)
    }

    fn ruta_bloqueo_global(&self) -> PathBuf {
        self.raiz.join(".bloqueo-preparacion")
    }

    fn asegurar_bloqueo_global(&self) -> Result<()> {
        let ruta = self.ruta_bloqueo_global();
        if !ruta.try_exists()? {
            let mut opciones = OpenOptions::new();
            opciones.write(true).create_new(true);
            #[cfg(unix)]
            opciones.mode(0o600);
            if let Err(error) = opciones.open(&ruta) {
                if !ruta.try_exists()? {
                    return Err(error).context("no se pudo crear el bloqueo global");
                }
            }
        }
        validar_fichero_privado(&ruta)
    }

    fn escribir_atomico(&self, recibo: &ReciboReserva) -> Result<()> {
        let directorio = self.directorio(&recibo.id_ejecucion);
        validar_directorio_privado(&directorio)?;
        let sufijo = CONTADOR_TEMPORALES.fetch_add(1, Ordering::Relaxed);
        let temporal = directorio.join(format!("recibo.json.tmp-{}-{sufijo}", std::process::id()));
        let contenido = serde_json::to_vec_pretty(recibo)?;
        let mut opciones = OpenOptions::new();
        opciones.write(true).create_new(true);
        #[cfg(unix)]
        opciones.mode(0o600);
        let mut archivo = opciones
            .open(&temporal)
            .with_context(|| format!("no se pudo crear {}", temporal.display()))?;
        archivo.write_all(&contenido)?;
        archivo.write_all(b"\n")?;
        archivo.sync_all()?;
        fs::rename(&temporal, self.ruta_recibo(&recibo.id_ejecucion))?;
        File::open(&directorio)?.sync_all()?;
        Ok(())
    }

    fn cargar_sin_bloqueo(&self, id_ejecucion: &Identificador) -> Result<ReciboReserva> {
        let directorio = self.directorio(id_ejecucion);
        validar_directorio_privado(&directorio)?;
        let ruta = self.ruta_recibo(id_ejecucion);
        let metadatos = fs::symlink_metadata(&ruta)?;
        if metadatos.file_type().is_symlink() || !metadatos.is_file() {
            bail!("el recibo no es un fichero ordinario");
        }
        let recibo: ReciboReserva =
            serde_json::from_reader(File::open(&ruta)?).context("el recibo no es JSON válido")?;
        if recibo.version != 1 || &recibo.id_ejecucion != id_ejecucion {
            bail!("el recibo no corresponde al identificador solicitado");
        }
        Ok(recibo)
    }
}

impl AlmacenRecibosReserva for AlmacenRecibosJson {
    fn bloquear_preparacion(&self) -> Result<Box<dyn GuardiaPreparacion + '_>> {
        let archivo = OpenOptions::new()
            .read(true)
            .write(true)
            .open(self.ruta_bloqueo_global())?;
        archivo.lock_exclusive()?;
        Ok(Box::new(archivo))
    }

    fn bloquear_mutacion(
        &self,
        id_ejecucion: &Identificador,
    ) -> Result<Box<dyn GuardiaMutacion + '_>> {
        Ok(Box::new(self.abrir_bloqueo_operacion(id_ejecucion)?))
    }

    fn existe(&self, id_ejecucion: &Identificador) -> Result<bool> {
        Ok(self.directorio(id_ejecucion).try_exists()?)
    }

    fn guardar_nuevo(&self, recibo: &ReciboReserva) -> Result<()> {
        let directorio = self.directorio(&recibo.id_ejecucion);
        let mut constructor = DirBuilder::new();
        #[cfg(unix)]
        constructor.mode(0o700);
        constructor.create(&directorio).with_context(|| {
            format!(
                "el identificador de ejecución ya está reservado: {}",
                recibo.id_ejecucion
            )
        })?;
        let ruta_bloqueo = directorio.join(".bloqueo");
        let mut opciones = OpenOptions::new();
        opciones.write(true).create_new(true);
        #[cfg(unix)]
        opciones.mode(0o600);
        opciones
            .open(&ruta_bloqueo)
            .with_context(|| format!("no se pudo crear el bloqueo {}", ruta_bloqueo.display()))?;
        let ruta_operacion = directorio.join(".operacion");
        let mut opciones_operacion = OpenOptions::new();
        opciones_operacion.write(true).create_new(true);
        #[cfg(unix)]
        opciones_operacion.mode(0o600);
        opciones_operacion.open(&ruta_operacion).with_context(|| {
            format!(
                "no se pudo crear el bloqueo de operación {}",
                ruta_operacion.display()
            )
        })?;
        let _bloqueo = self.abrir_bloqueo(&recibo.id_ejecucion)?;
        self.escribir_atomico(recibo)
    }

    fn cargar(&self, id_ejecucion: &Identificador) -> Result<ReciboReserva> {
        self.cargar_sin_bloqueo(id_ejecucion)
    }

    fn listar(&self) -> Result<Vec<ReciboReserva>> {
        let mut recibos = Vec::new();
        for entrada in fs::read_dir(&self.raiz)? {
            let entrada = entrada?;
            if entrada.file_name() == ".bloqueo-preparacion" {
                validar_fichero_privado(&entrada.path())?;
                continue;
            }
            let metadatos = fs::symlink_metadata(entrada.path())?;
            if metadatos.file_type().is_symlink() || !metadatos.is_dir() {
                bail!("la raíz de recibos contiene una entrada no permitida");
            }
            let nombre = entrada
                .file_name()
                .into_string()
                .map_err(|_| anyhow::anyhow!("un identificador de recibo no es UTF-8"))?;
            let id = Identificador::nuevo(nombre)
                .context("la raíz contiene un identificador de recibo no válido")?;
            recibos.push(self.cargar_sin_bloqueo(&id)?);
        }
        recibos.sort_by(|a, b| a.id_ejecucion.cmp(&b.id_ejecucion));
        Ok(recibos)
    }

    fn actualizar(&self, recibo: &ReciboReserva) -> Result<()> {
        let _bloqueo = self.abrir_bloqueo(&recibo.id_ejecucion)?;
        let anterior = self.cargar_sin_bloqueo(&recibo.id_ejecucion)?;
        if anterior.id_plantilla != recibo.id_plantilla
            || anterior.id_instancia != recibo.id_instancia
            || anterior.sistema != recibo.sistema
            || anterior.creado_en_unix_ms != recibo.creado_en_unix_ms
        {
            bail!("se intentó cambiar la identidad inmutable del recibo");
        }
        if anterior.revision.checked_add(1) != Some(recibo.revision) {
            bail!("conflicto de revisión al actualizar el recibo");
        }
        self.escribir_atomico(recibo)
    }
}

impl GuardiaPreparacion for File {}
impl GuardiaMutacion for File {}

pub struct RelojSistema;

impl Reloj for RelojSistema {
    fn ahora_unix_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX)
    }
}

fn validar_directorio_privado(ruta: &Path) -> Result<()> {
    let metadatos = fs::symlink_metadata(ruta)
        .with_context(|| format!("no se pudo inspeccionar {}", ruta.display()))?;
    if metadatos.file_type().is_symlink() || !metadatos.is_dir() {
        bail!("la raíz privada no es un directorio ordinario");
    }
    #[cfg(unix)]
    {
        if metadatos.mode() & 0o077 != 0 {
            bail!("la raíz privada permite acceso a grupo u otros usuarios");
        }
        // SAFETY: geteuid no recibe punteros ni tiene precondiciones.
        if metadatos.uid() != unsafe { libc::geteuid() } {
            bail!("la raíz privada no pertenece al usuario actual");
        }
    }
    Ok(())
}

fn validar_fichero_privado(ruta: &Path) -> Result<()> {
    let metadatos = fs::symlink_metadata(ruta)?;
    if metadatos.file_type().is_symlink() || !metadatos.is_file() {
        bail!("el fichero de control no es ordinario");
    }
    #[cfg(unix)]
    {
        if metadatos.mode() & 0o077 != 0 {
            bail!("el fichero de control permite acceso a grupo u otros usuarios");
        }
        // SAFETY: geteuid no recibe punteros ni tiene precondiciones.
        if metadatos.uid() != unsafe { libc::geteuid() } {
            bail!("el fichero de control no pertenece al usuario actual");
        }
    }
    Ok(())
}

#[cfg(test)]
mod pruebas {
    use super::*;
    use crate::dominio::plantilla::SistemaInvitado;
    use crate::dominio::reserva::EstadoReserva;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn recibo(id: &str) -> ReciboReserva {
        ReciboReserva::nuevo(
            Identificador::nuevo(id).unwrap(),
            Identificador::nuevo("windows-analisis").unwrap(),
            SistemaInvitado::Windows,
            10,
        )
        .unwrap()
    }

    fn almacen_temporal() -> (tempfile::TempDir, AlmacenRecibosJson) {
        let temporal = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        fs::set_permissions(temporal.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let almacen = AlmacenRecibosJson::nuevo(temporal.path()).unwrap();
        (temporal, almacen)
    }

    #[test]
    fn reserva_y_actualiza_por_revision() {
        let (_temporal, almacen) = almacen_temporal();
        let mut valor = recibo("ejecucion-uno");
        almacen.guardar_nuevo(&valor).unwrap();
        assert_eq!(almacen.cargar(&valor.id_ejecucion).unwrap(), valor);
        valor.iniciar(20).unwrap();
        almacen.actualizar(&valor).unwrap();
        assert_eq!(almacen.cargar(&valor.id_ejecucion).unwrap(), valor);
        assert_eq!(valor.estado, EstadoReserva::EnEjecucion);
    }

    #[test]
    fn rechaza_una_escritura_obsoleta() {
        let (_temporal, almacen) = almacen_temporal();
        let original = recibo("ejecucion-dos");
        almacen.guardar_nuevo(&original).unwrap();
        let mut primero = original.clone();
        let mut segundo = original;
        primero.iniciar(20).unwrap();
        segundo.iniciar(21).unwrap();
        almacen.actualizar(&primero).unwrap();
        assert!(almacen.actualizar(&segundo).is_err());
    }
}
