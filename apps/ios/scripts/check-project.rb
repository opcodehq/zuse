require 'xcodeproj'
require 'set'
root = File.expand_path('..', __dir__)
project = Xcodeproj::Project.open(File.join(root, 'Zuse.xcodeproj'))
expected = Dir.glob(File.join(root, 'App/*.swift')).map { |path| File.basename(path) }.to_set
%w[Zuse ZuseMac].each do |name|
  target = project.targets.find { |item| item.name == name }
  abort "Missing #{name} app target" unless target
  actual = target.source_build_phase.files.map { |file| file.file_ref.path }.to_set
  abort "#{name} source membership is stale: #{(expected ^ actual).to_a.join(', ')}" unless expected == actual
  target.resources_build_phase.files.each do |file|
    abort "Missing resource #{file.file_ref.path}" unless File.exist?(file.file_ref.real_path)
  end
  target.build_configurations.each do |config|
    plist = config.build_settings['INFOPLIST_FILE']
    abort "Missing #{name} Info.plist" unless plist && File.exist?(File.join(root, plist))
    values = Xcodeproj::Plist.read_from_path(File.join(root, plist))
    abort "Missing cloud configuration for #{name}" unless values['WorkOSClientID'] && values['ZuseAPIURL']
  end
  puts "#{name}: #{actual.size} Swift sources and resources present"
end
