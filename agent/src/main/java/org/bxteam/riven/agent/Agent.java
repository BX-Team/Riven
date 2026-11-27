package org.bxteam.riven.agent;

import java.lang.instrument.ClassFileTransformer;
import java.lang.instrument.Instrumentation;
import java.security.CodeSource;
import java.security.ProtectionDomain;
import java.util.EnumSet;
import java.util.Set;

/**
 * Riven in the game, whatever the version or loader: LAN worlds announced by Riven always show, and with BX ID
 * skins on, authlib is patched as the game loads it.
 */
public final class Agent {
    private static final Set<Patch.Kind> patched = EnumSet.noneOf(Patch.Kind.class);

    private Agent() {}

    public static void premain(String args, Instrumentation instrumentation) {
        boolean skins = !System.getProperty(Hooks.API, "").isEmpty();
        instrumentation.addTransformer(new ClassFileTransformer() {
            @Override
            public byte[] transform(
                    ClassLoader loader, String name, Class<?> redefined, ProtectionDomain domain, byte[] bytes) {
                if (name == null || name.startsWith("java/") || name.startsWith("jdk/") || name.startsWith("sun/"))
                    return null;
                try {
                    if (name.startsWith(Patch.AUTHLIB)) {
                        if (!skins) return null;
                        Patch patch = Patch.apply(bytes);
                        if (!patch.hooks.isEmpty()) found(patch, domain);
                        return patch.bytes;
                    }
                    byte[] lan = Lan.apply(bytes);
                    if (lan != null) System.err.println("[riven-agent] Riven LAN worlds are on: " + name + " patched");
                    return lan;
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
