@ECHO OFF
SET APP_HOME=%~dp0
"%JAVA_HOME%\bin\java.exe" -Dorg.gradle.appname=gradlew -classpath "%APP_HOME%\gradle\wrapper\gradle-wrapper.jar" org.gradle.wrapper.GradleWrapperMain %*
