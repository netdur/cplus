/* tiny_artifact — upstream C source.
 *
 * This file is the package author's source, NOT compiled by cpc.
 * The build artifact (libtiny_artifact.a) is produced by upstream/build.sh
 * and lives under src/lib/<host-triple>/. Consumers see only the archive.
 *
 * Living it under upstream/ (outside src/) keeps absolutely clear which
 * files cpc treats as C+ sources.
 */

int tiny_artifact_double(int n) {
    return n * 2;
}
