package org.bxteam.riven.agent;

import java.lang.instrument.ClassFileTransformer;
import java.lang.instrument.Instrumentation;
import java.security.CodeSource;
import java.security.ProtectionDomain;
import java.util.EnumSet;
import java.util.Set;

/** Riven's skins in the game: patches authlib as the game loads it, whatever the version or loader. */
public final class Agent {
    private static final Set<Patch.Kind> patched = EnumSet.noneOf(Patch.Kind.class);

    private Agent() {}

    public static void premain(String args, Instrumentation instrumentation) {
        instrumentation.addTransformer(new ClassFileTransformer() {
            @Override
            public byte[] transform(
                    ClassLoader loader, String name, Class<?> redefined, ProtectionDomain domain, byte[] bytes) {
                if (name == null || !name.startsWith(Patch.AUTHLIB)) return null;
                try {
                    Patch patch = Patch.apply(bytes);
                    if (!patch.hooks.isEmpty()) found(patch, domain);
                    return patch.bytes;
                } catch (Throwable e) {
                    // A class the agent cannot read stays as it was; the game must still start.
                    System.err.println("[riven-agent] cannot patch " + name + ": " + e);
                    return null;
                }
            }
        });
    }

    /** One line once every kind of hook is in: the classes holding them load at different times. */
    private static synchronized void found(Patch patch, ProtectionDomain domain) {
        if (patched.size() == Patch.Kind.values().length) return;
        for (String hook : patch.hooks) patched.add(Patch.Kind.valueOf(hook.split(" ")[1].toUpperCase()));
        if (patched.size() == Patch.Kind.values().length)
            System.err.println("[riven-agent] BX ID skins are on: " + jar(domain) + " patched");
    }

    private static String jar(ProtectionDomain domain) {
        CodeSource source = domain == null ? null : domain.getCodeSource();
        if (source == null || source.getLocation() == null) return "authlib";
        // Loaders wrap the location their own way (`union:`, `jar:`, `#123!/`); the file name is inside all of them.
        java.util.regex.Matcher file =
                java.util.regex.Pattern.compile("authlib-[0-9.]+\\.jar").matcher(source.getLocation().toString());
        return file.find() ? file.group() : "authlib";
    }
}
