// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

import javax.inject.Inject

import org.gradle.api.DefaultTask
import org.gradle.api.file.ConfigurableFileCollection
import org.gradle.api.file.DirectoryProperty
import org.gradle.api.provider.ListProperty
import org.gradle.api.tasks.CacheableTask
import org.gradle.api.tasks.Input
import org.gradle.api.tasks.InputFiles
import org.gradle.api.tasks.Internal
import org.gradle.api.tasks.OutputDirectory
import org.gradle.api.tasks.PathSensitive
import org.gradle.api.tasks.PathSensitivity
import org.gradle.api.tasks.TaskAction
import org.gradle.process.ExecOperations

/**
 * Java sources written by {@code vento-codegen} ({@code backends/codegen}) out
 * of what the Rust side says, for a module to compile as a generated source
 * directory: {@code variant.sources.java.addGeneratedSourceDirectory}, which
 * is also where this task's output directory is decided.
 *
 * <p>Unlike {@link CargoBuildTask} this one is checked and cached the ordinary
 * way, because what it reads is small and known: the generator's sources and
 * the crates it is built from, the lock file and the toolchain pin. Its
 * output is a function of those and of {@link #getArguments} alone — the
 * generator writes no time, no path and nothing in a map's order — so a cached
 * output is the output, and two checkouts produce the same files.
 *
 * <p>The generator is run with {@code cargo run} on the host, in the
 * workspace's own target directory, under the environment {@code
 * CargoBuildTask} scrubs, so that an ambient {@code RUSTFLAGS} cannot change
 * what it is.
 */
@CacheableTask
abstract class GenerateJavaTask extends DefaultTask {

    /** What to generate: {@code vento-codegen}'s arguments, less the output directory. */
    @Input
    abstract ListProperty<String> getArguments()

    /** Everything the generator is built from. */
    @InputFiles
    @PathSensitive(PathSensitivity.RELATIVE)
    abstract ConfigurableFileCollection getGeneratorSources()

    /** The workspace root, which is also the Gradle root. */
    @Internal
    abstract DirectoryProperty getWorkspaceDir()

    @OutputDirectory
    abstract DirectoryProperty getOutputDir()

    @Inject
    abstract ExecOperations getExecOperations()

    @TaskAction
    void generate() {
        final outDir = outputDir.get().asFile
        outDir.deleteDir()
        outDir.mkdirs()

        final env = CargoBuildTask.scrubbedEnvironment()
        env.put('CARGO_TERM_PROGRESS_WHEN', 'never')
        env.put('CARGO_TERM_COLOR', 'never')

        final cmd = ['cargo', 'run', '--quiet', '--locked', '--package', 'remotedesktop-codegen', '--']
        cmd.addAll(arguments.get())
        cmd.add(outDir.absolutePath)
        execOperations.exec {
            it.workingDir = workspaceDir.get().asFile
            it.commandLine = cmd
            it.environment = env
        }
    }
}
