//! Encode and apply Xdelta patches through the external `xdelta3` command.
//!
//! [`xdelta_compress`] produces a patch from two files; [`xdelta_decompress`] applies
//! a streamed patch to a source file and writes decoded output. Bundle payload
//! acquisition and verification are handled separately by the bundle reader.

use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::process::Stdio;

use reportify::ResultExt;
use reportify::bail;
use tracing::Level;
use tracing::trace;

use crate::BundleResult;

/// Apply a streamed patch against `source`, writing reconstructed bytes to `output`.
///
/// For a patch produced by [`xdelta_compress`], `source` must contain the contents
/// of `old`; applying the patch reconstructs the contents of `new`.
///
/// Runs `xdelta3 -d -c -s SOURCE`, feeding `patch` to stdin and copying stdout to
/// `output` concurrently on scoped threads. The `xdelta3` executable must be
/// available on `PATH`. Waits for the command and both copy operations to finish.
///
/// Returns an error if the command cannot be run, exits unsuccessfully, or a data
/// transfer fails. Bytes already written to `output` remain on failure. Validation
/// against an expected output hash belongs to the caller.
#[tracing::instrument(level = Level::DEBUG, skip(patch, output))]
pub fn xdelta_decompress<R, W>(source: &Path, patch: &mut R, output: &mut W) -> BundleResult<()>
where
    R: Read + Send,
    W: Write + Send,
{
    let mut child = Command::new("xdelta3")
        .arg("-d")
        .arg("-c")
        .arg("-s")
        .arg(source)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .whatever("unable to spawn xdelta")?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| reportify::whatever!("xdelta stdin pipe is unavailable"))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| reportify::whatever!("xdelta stdout pipe is unavailable"))?;
    let (exit_status, input_result, output_result) = std::thread::scope(|scope| {
        let input_thread = scope.spawn(move || {
            trace!("feeding patch to xdelta");
            let result = std::io::copy(patch, &mut stdin);
            trace!(?result, "done feeding patch to xdelta");
            result
        });
        let output_thread = scope.spawn(move || std::io::copy(&mut stdout, output));
        let exit_status = child.wait().whatever("error running xdelta")?;
        let input_result = input_thread
            .join()
            .map_err(|_| reportify::whatever!("xdelta input worker panicked"))?;
        let output_result = output_thread
            .join()
            .map_err(|_| reportify::whatever!("xdelta output worker panicked"))?;
        Ok::<_, reportify::Report<crate::BundleError>>((exit_status, input_result, output_result))
    })?;
    input_result.whatever("unable to feed patch data to xdelta")?;
    output_result.whatever("unable to copy decompressed xdelta output")?;
    if !exit_status.success() {
        bail!(
            "xdelta exited with non-zero return code: {:?}",
            exit_status.code()
        );
    }
    Ok(())
}

/// Encode the differences between the files `old` and `new` into `patch`.
///
/// Applying the patch with [`xdelta_decompress`], using `old` as the source,
/// reconstructs the contents of `new`.
///
/// Runs `xdelta3 -e -s OLD NEW PATCH` and waits for it to finish. The `xdelta3`
/// executable must be available on `PATH`. Returns an error if the command cannot
/// be run or exits unsuccessfully.
#[tracing::instrument(level = Level::DEBUG)]
pub fn xdelta_compress(old: &Path, new: &Path, patch: &Path) -> BundleResult<()> {
    let mut child = Command::new("xdelta3")
        .arg("-e")
        .arg("-s")
        .arg(old)
        .arg(new)
        .arg(patch)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .whatever("unable to spawn xdelta")?;
    let exit_status = child.wait().whatever("error running xdelta")?;
    if !exit_status.success() {
        bail!(
            "xdelta exited with non-zero return code: {:?}",
            exit_status.code()
        );
    }
    Ok(())
}
