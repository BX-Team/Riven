package org.bxteam.riven.agent;

import static org.junit.jupiter.api.Assertions.*;

import java.io.File;
import java.io.IOException;
import java.io.InputStream;
import java.lang.reflect.Constructor;
import java.lang.reflect.Method;
import java.lang.reflect.Modifier;
import java.net.HttpURLConnection;
import java.net.Proxy;
import java.net.URL;
import java.net.URI;
import java.net.URLClassLoader;
import java.nio.charset.StandardCharsets;
import java.security.KeyPair;
import java.security.KeyPairGenerator;
import java.security.Signature;
import java.util.*;
import java.util.jar.JarFile;
import java.util.stream.Stream;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.params.ParameterizedTest;
import org.junit.jupiter.params.provider.MethodSource;

/** Patches every authlib Minecraft 1.12.2 and later ship and checks the hooks work in each. */
class AuthlibTest {
    private static final String API = "https://skins.example.test/v1/skins";
    private static KeyPair ours;
    private static KeyPair stranger;
    /** Stands in for Mojang's key where a test passes one in. */
    private static KeyPair mojang;

    @BeforeAll
    static void keys() throws Exception {
        KeyPairGenerator generator = KeyPairGenerator.getInstance("RSA");
        generator.initialize(2048);
        ours = generator.generateKeyPair();
        stranger = generator.generateKeyPair();
        mojang = generator.generateKeyPair();
        System.setProperty(Hooks.API, API);
        System.setProperty(Hooks.KEY, Base64.getEncoder().encodeToString(ours.getPublic().getEncoded()));
    }

    static Stream<String> versions() {
        return Arrays.stream(System.getProperty("authlibs").split(";")).map(entry -> entry.split("=", 2)[0]);
    }

    @ParameterizedTest
    @MethodSource("versions")
    void everyKindOfHookIsFound(String version) throws IOException {
        Set<Patch.Kind> found = EnumSet.noneOf(Patch.Kind.class);
        try (JarFile jar = new JarFile(authlib(version))) {
            for (var entry : Collections.list(jar.entries())) {
                if (!entry.getName().startsWith(Patch.AUTHLIB) || !entry.getName().endsWith(".class")) continue;
                try (InputStream in = jar.getInputStream(entry)) {
                    Patch patch = Patch.apply(in.readAllBytes());
                    for (String hook : patch.hooks)
                        found.add(Patch.Kind.valueOf(hook.split(" ")[1].toUpperCase()));
                    if (patch.bytes != null)
                        assertFalse(new String(patch.bytes, StandardCharsets.ISO_8859_1).contains("org/bxteam/"),
                                entry.getName() + " links to the agent's classes, which game loaders may hide");
                }
            }
        }
        assertEquals(EnumSet.allOf(Patch.Kind.class), found);
    }

    @ParameterizedTest
    @MethodSource("versions")
    void ourTexturesPassAndOthersDoNot(String version) throws Exception {
        try (Patched loader = new Patched(version)) {
            Class<?> property = loader.loadClass("com.mojang.authlib.properties.Property");
            Constructor<?> make = property.getConstructor(String.class, String.class, String.class);
            String value = Base64.getEncoder().encodeToString("{\"textures\":{}}".getBytes(StandardCharsets.UTF_8));
            Object signed = make.newInstance("textures", value, sign(ours, value));
            Object forged = make.newInstance("textures", value, sign(stranger, value));
            Object otherName = make.newInstance("other", value, sign(ours, value));

            Method checkOld = property.getMethod("isSignatureValid", java.security.PublicKey.class);
            assertTrue((boolean) checkOld.invoke(signed, mojang.getPublic()));
            assertFalse((boolean) checkOld.invoke(forged, mojang.getPublic()));
            assertFalse((boolean) checkOld.invoke(otherName, mojang.getPublic()));

            Object keyInfo = mojangKeyInfo(loader);
            if (keyInfo != null) {
                Method validate = keyInfo.getClass().getMethod("validateProperty", property);
                validate.setAccessible(true);
                assertTrue((boolean) validate.invoke(keyInfo, signed));
                assertFalse((boolean) validate.invoke(keyInfo, forged));
            }
        }
    }

    @ParameterizedTest
    @MethodSource("versions")
    void ourTextureHostIsAllowed(String version) throws Exception {
        try (Patched loader = new Patched(version)) {
            Method check = staticDomainCheck(loader);
            if (check == null) return;
            assertTrue((boolean) check.invoke(null, API + "/textures/u/classic/" + "a".repeat(64) + ".png"));
            assertTrue((boolean) check.invoke(null, "http://textures.minecraft.net/texture/abc"));
            assertFalse((boolean) check.invoke(null, "https://evil.example.test/a.png"));
        }
    }

    @ParameterizedTest
    @MethodSource("versions")
    void sessionCallsReachRiven(String version) throws Exception {
        try (Patched loader = new Patched(version)) {
            URL hasJoined = URI.create("https://sessionserver.mojang.com/session/minecraft/hasJoined?username=a&serverId=b").toURL();
            boolean tried = false;
            for (Object http : httpClients(loader)) {
                Method create = find(http.getClass(), "createUrlConnection");
                create.setAccessible(true);
                HttpURLConnection connection = (HttpURLConnection) create.invoke(http, hasJoined);
                assertEquals(API + "/session/minecraft/hasJoined?username=a&serverId=b", connection.getURL().toString());
                tried = true;
            }
            assertTrue(tried, "no HTTP client to try");
        }
    }

    private static String sign(KeyPair pair, String value) throws Exception {
        Signature signature = Signature.getInstance("SHA1withRSA");
        signature.initSign(pair.getPrivate());
        signature.update(value.getBytes(StandardCharsets.UTF_8));
        return Base64.getEncoder().encodeToString(signature.sign());
    }

    /** Mojang's own key as authlib 3.5 and later hold it, from the copy inside the jar. */
    private static Object mojangKeyInfo(ClassLoader loader) throws Exception {
        for (String name : List.of(
                "com.mojang.authlib.yggdrasil.YggdrasilServicesKeyInfo", "com.mojang.authlib.services.MinecraftServicesKeyInfo")) {
            Class<?> type;
            try {
                type = loader.loadClass(name);
            } catch (ClassNotFoundException e) {
                continue;
            }
            for (Method method : type.getDeclaredMethods()) {
                if (!Modifier.isStatic(method.getModifiers())) continue;
                method.setAccessible(true);
                if (method.getName().equals("createFromResources") && method.getParameterCount() == 0)
                    return method.invoke(null);
                if (method.getName().equals("parse") && Arrays.equals(method.getParameterTypes(), new Class<?>[] {byte[].class}))
                    return method.invoke(null, (Object) mojang.getPublic().getEncoded());
            }
        }
        return null;
    }

    private static Method staticDomainCheck(ClassLoader loader) throws Exception {
        for (String name : List.of(
                "com.mojang.authlib.yggdrasil.TextureUrlChecker", "com.mojang.authlib.yggdrasil.YggdrasilMinecraftSessionService")) {
            Class<?> type;
            try {
                type = loader.loadClass(name);
            } catch (ClassNotFoundException e) {
                continue;
            }
            for (Method method : type.getDeclaredMethods()) {
                if (Modifier.isStatic(method.getModifiers())
                        && Patch.kind(method.getName(), Type.descriptor(method)) == Patch.Kind.DOMAIN) {
                    method.setAccessible(true);
                    return method;
                }
            }
        }
        return null;
    }

    /** The objects that open authlib's connections: MinecraftClient, and the old authentication service. */
    private static List<Object> httpClients(ClassLoader loader) throws Exception {
        List<Object> clients = new ArrayList<>();
        try {
            clients.add(loader.loadClass("com.mojang.authlib.minecraft.client.MinecraftClient")
                    .getConstructor(String.class, Proxy.class)
                    .newInstance("token", Proxy.NO_PROXY));
        } catch (ClassNotFoundException e) {
            // Before authlib 2.3.
        }
        try {
            Class<?> service = loader.loadClass("com.mojang.authlib.yggdrasil.YggdrasilAuthenticationService");
            if (find(service, "createUrlConnection") != null) {
                for (Constructor<?> constructor : service.getConstructors()) {
                    Object[] args = Arrays.stream(constructor.getParameterTypes())
                            .map(type -> type == Proxy.class ? Proxy.NO_PROXY : type == String.class ? "token" : null)
                            .toArray();
                    try {
                        clients.add(constructor.newInstance(args));
                        break;
                    } catch (Exception e) {
                        // Another constructor may take what we have.
                    }
                }
            }
        } catch (ClassNotFoundException e) {
            // Gone in authlib 10.
        }
        return clients;
    }

    private static Method find(Class<?> type, String name) {
        for (Class<?> at = type; at != null; at = at.getSuperclass())
            for (Method method : at.getDeclaredMethods()) if (method.getName().equals(name)) return method;
        return null;
    }

    private static File authlib(String version) {
        return files(version).stream()
                .filter(file -> file.getName().equals("authlib-" + version + ".jar"))
                .findFirst()
                .orElseThrow();
    }

    private static List<File> files(String version) {
        for (String entry : System.getProperty("authlibs").split(";")) {
            String[] parts = entry.split("=", 2);
            if (parts[0].equals(version))
                return Arrays.stream(parts[1].split(File.pathSeparator)).map(File::new).toList();
        }
        throw new IllegalArgumentException(version);
    }

    /** Loads one authlib and its libraries, patching it as the agent would; like Fabric's Knot, it cannot see the agent's classes. */
    private static final class Patched extends URLClassLoader {
        Patched(String version) {
            super(files(version).stream().map(Patched::url).toArray(URL[]::new), ClassLoader.getPlatformClassLoader());
        }

        private static URL url(File file) {
            try {
                return file.toURI().toURL();
            } catch (Exception e) {
                throw new IllegalStateException(e);
            }
        }

        @Override
        protected Class<?> loadClass(String name, boolean resolve) throws ClassNotFoundException {
            if (!name.startsWith("com.mojang.authlib.")) return super.loadClass(name, resolve);
            synchronized (getClassLoadingLock(name)) {
                Class<?> loaded = findLoadedClass(name);
                if (loaded != null) return loaded;
                try (InputStream in = getResourceAsStream(name.replace('.', '/') + ".class")) {
                    if (in == null) throw new ClassNotFoundException(name);
                    byte[] bytes = in.readAllBytes();
                    Patch patch = Patch.apply(bytes);
                    byte[] used = patch.bytes == null ? bytes : patch.bytes;
                    return defineClass(name, used, 0, used.length);
                } catch (IOException e) {
                    throw new ClassNotFoundException(name, e);
                }
            }
        }
    }

    private static final class Type {
        static String descriptor(Method method) {
            return org.objectweb.asm.Type.getMethodDescriptor(method);
        }
    }
}
